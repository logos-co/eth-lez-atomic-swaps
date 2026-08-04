#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
fixture="$repo_root/tests/fixtures/protocol-v2-eip712.json"

for command in cast jq; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "error: $command is required to verify $fixture" >&2
    exit 1
  fi
done

json() {
  jq -er "$1" "$fixture"
}

expect_equal() {
  local label=$1
  local actual=$2
  local expected=$3

  if [[ "$actual" != "$expected" ]]; then
    printf 'error: %s mismatch\n  actual:   %s\n  expected: %s\n' \
      "$label" "$actual" "$expected" >&2
    exit 1
  fi
}

domain_type=$(json '.domain.type_string')
domain_type_hash=$(cast keccak "$domain_type")
expect_equal "domain type hash" "$domain_type_hash" "$(json '.domain.type_hash')"

chain_id=$(json '.domain.chain_id')
contract=$(json '.domain.verifying_contract')
lez_chain=$(json '.domain.lez_chain_id')
lez_program=$(json '.domain.lez_htlc_program_id')
code_hash=$(json '.domain.ethereum_htlc_code_hash')

salt=$(cast keccak "$(cast abi-encode \
  'f(string,uint256,bytes32,bytes32)' \
  'logos.atomic-swaps' 2 "$lez_chain" "$lez_program")")
expect_equal "domain salt" "$salt" "$(json '.domain.salt')"

domain_separator=$(cast keccak "$(cast abi-encode \
  'f(bytes32,bytes32,bytes32,uint256,address,bytes32)' \
  "$domain_type_hash" \
  "$(cast keccak "$(json '.domain.name')")" \
  "$(cast keccak "$(json '.domain.version')")" \
  "$chain_id" "$contract" "$salt")")
expect_equal "domain separator" "$domain_separator" "$(json '.domain.separator')"

expect_equal "offer chain ID" "$(json '.offer.typed_values.ethereum_chain_id')" "$chain_id"
expect_equal "offer contract" "$(json '.offer.typed_values.ethereum_htlc')" "$contract"
expect_equal "offer code hash" "$(json '.offer.typed_values.ethereum_htlc_code_hash')" "$code_hash"
expect_equal "offer LEZ chain" "$(json '.offer.typed_values.lez_chain_id')" "$lez_chain"
expect_equal "offer LEZ program" "$(json '.offer.typed_values.lez_htlc_program_id')" "$lez_program"

offer_type=$(json '.offer.primary_type')
offer_type_hash=$(cast keccak "$offer_type")
expect_equal "offer type hash" "$offer_type_hash" "$(json '.offer.expected.type_hash')"

offer_struct_hash=$(cast keccak "$(cast abi-encode \
  'f(bytes32,bytes32,uint256,address,bytes32,bytes32,bytes32,address,bytes32,uint256,uint128,uint64,uint64,uint64,uint32,uint64,uint64)' \
  "$offer_type_hash" \
  "$(json '.offer.typed_values.offer_id')" \
  "$chain_id" "$contract" "$code_hash" "$lez_chain" "$lez_program" \
  "$(json '.offer.typed_values.maker_eth_address')" \
  "$(json '.actors.maker.lez_account_bytes')" \
  "$(json '.offer.typed_values.eth_amount_wei')" \
  "$(json '.offer.typed_values.lez_amount')" \
  "$(json '.offer.typed_values.lez_timelock_duration_sec')" \
  "$(json '.offer.typed_values.eth_timelock_duration_sec')" \
  "$(json '.offer.typed_values.min_timelock_margin_sec')" \
  "$(json '.offer.typed_values.max_fills')" \
  "$(json '.offer.typed_values.issued_at')" \
  "$(json '.offer.typed_values.expires_at')")")
expect_equal "offer struct hash" "$offer_struct_hash" "$(json '.offer.expected.struct_hash')"

offer_digest=$(cast keccak "$(cast concat-hex 0x1901 "$domain_separator" "$offer_struct_hash")")
expect_equal "offer digest" "$offer_digest" "$(json '.offer.expected.digest')"
expect_equal "accept offer digest" "$(json '.accept.typed_values.offer_digest')" "$offer_digest"

maker_key=$(json '.actors.maker.private_key')
maker_address=$(json '.actors.maker.address')
derived_maker=$(cast wallet address --private-key "$maker_key" | tr '[:upper:]' '[:lower:]')
expect_equal "maker key address" "$derived_maker" "$maker_address"
expect_equal "offer recovered signer" "$(json '.offer.expected.recovered_signer')" "$maker_address"
offer_signature=$(cast wallet sign --no-hash --private-key "$maker_key" "$offer_digest")
expect_equal "offer signature" "$offer_signature" "$(json '.offer.expected.signature')"
cast wallet verify --no-hash --address "$maker_address" \
  "$offer_digest" "$offer_signature" >/dev/null

taker_address=$(json '.actors.taker.address')
taker_lez=$(json '.actors.taker.lez_account_bytes')
swap_id=$(cast keccak "$(cast abi-encode --packed \
  'f(address,address,uint256,bytes32,uint256,bytes32)' \
  "$taker_address" "$maker_address" \
  "$(json '.accept.typed_values.eth_amount_wei')" \
  "$(json '.accept.typed_values.hashlock')" \
  "$(json '.accept.typed_values.eth_refund_after')" \
  "$taker_lez")")
expect_equal "ETH swap ID" "$swap_id" "$(json '.accept.typed_values.eth_swap_id')"

expect_equal "accept chain ID" "$(json '.accept.typed_values.ethereum_chain_id')" "$chain_id"
expect_equal "accept contract" "$(json '.accept.typed_values.ethereum_htlc')" "$contract"
expect_equal "accept code hash" "$(json '.accept.typed_values.ethereum_htlc_code_hash')" "$code_hash"
expect_equal "accept LEZ chain" "$(json '.accept.typed_values.lez_chain_id')" "$lez_chain"
expect_equal "accept LEZ program" "$(json '.accept.typed_values.lez_htlc_program_id')" "$lez_program"

accept_type=$(json '.accept.primary_type')
accept_type_hash=$(cast keccak "$accept_type")
expect_equal "accept type hash" "$accept_type_hash" "$(json '.accept.expected.type_hash')"

accept_struct_hash=$(cast keccak "$(cast abi-encode \
  'f(bytes32,bytes32,bytes32,uint256,address,bytes32,bytes32,bytes32,address,bytes32,uint256,uint128,bytes32,bytes32,bytes32,address,bytes32,uint64,uint64,uint64,uint64)' \
  "$accept_type_hash" \
  "$(json '.accept.typed_values.offer_id')" "$offer_digest" \
  "$chain_id" "$contract" "$code_hash" "$lez_chain" "$lez_program" \
  "$(json '.accept.typed_values.maker_eth_address')" \
  "$(json '.actors.maker.lez_account_bytes')" \
  "$(json '.accept.typed_values.eth_amount_wei')" \
  "$(json '.accept.typed_values.lez_amount')" \
  "$swap_id" \
  "$(json '.accept.typed_values.eth_lock_tx_hash')" \
  "$(json '.accept.typed_values.hashlock')" \
  "$taker_address" "$taker_lez" \
  "$(json '.accept.typed_values.eth_refund_after')" \
  "$(json '.accept.typed_values.lez_refund_after')" \
  "$(json '.accept.typed_values.issued_at')" \
  "$(json '.accept.typed_values.expires_at')")")
expect_equal "accept struct hash" "$accept_struct_hash" "$(json '.accept.expected.struct_hash')"

accept_digest=$(cast keccak "$(cast concat-hex 0x1901 "$domain_separator" "$accept_struct_hash")")
expect_equal "accept digest" "$accept_digest" "$(json '.accept.expected.digest')"

taker_key=$(json '.actors.taker.private_key')
derived_taker=$(cast wallet address --private-key "$taker_key" | tr '[:upper:]' '[:lower:]')
expect_equal "taker key address" "$derived_taker" "$taker_address"
expect_equal "accept recovered signer" "$(json '.accept.expected.recovered_signer')" "$taker_address"
accept_signature=$(cast wallet sign --no-hash --private-key "$taker_key" "$accept_digest")
expect_equal "accept signature" "$accept_signature" "$(json '.accept.expected.signature')"
cast wallet verify --no-hash --address "$taker_address" \
  "$accept_digest" "$accept_signature" >/dev/null

echo "protocol-v2 fixture verified: domain, offer, swap ID, acceptance, and signatures"
