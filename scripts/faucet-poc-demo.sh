#!/usr/bin/env bash
#
# Drive one full claim against a RUNNING in-house drip faucet:
#
#   GET  /challenge?address=…   ->  {seed, difficulty_bits, expires_at}
#   solve                       ->  a u128 whose SHA256(seed||solution) has
#                                   difficulty_bits leading zero bits
#   POST /drip {address, pow_solution}  ->  {tx_hash, …}
#
# This is the exact flow the app's "Get test ETH" button runs, done by hand —
# the solve even uses the same crate (`eth-faucet --solve`), so a demo that
# passes here is evidence about the shipped scheme rather than about a shell
# reimplementation of it.
#
#   make faucet-poc-run                                   # in one terminal
#   make faucet-poc-demo ADDRESS=0xYourAddress            # in another
#
# See README-poc.md.

set -euo pipefail

ADDRESS="${1:-}"
BASE="${2:-http://127.0.0.1:8787}"
BASE="${BASE%/}"

if [[ -z "$ADDRESS" ]]; then
    echo "usage: $0 <0x-address> [faucet-url]" >&2
    exit 2
fi
# Fail here rather than making the operator read a server-side "bad_address".
if [[ ! "$ADDRESS" =~ ^0[xX][0-9a-fA-F]{40}$ ]]; then
    echo "'$ADDRESS' is not an Ethereum address (0x + 40 hex chars)" >&2
    exit 2
fi

for tool in curl python3; do
    command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required" >&2; exit 2; }
done

# One tiny JSON reader, so the script needs no jq. Prints the value at a
# top-level key, or exits non-zero if the body is not an object with that key.
json_get() {
    python3 -c '
import json, sys
try:
    body = json.load(sys.stdin)
except json.JSONDecodeError:
    sys.exit(1)
value = body
for key in sys.argv[1:]:
    if not isinstance(value, dict) or key not in value:
        sys.exit(1)
    value = value[key]
print(value)
' "$@"
}

say() { printf '\n\033[1m%s\033[0m\n' "$*"; }

say "1/4  Is the faucet up?  GET $BASE/health"
health="$(curl -fsS "$BASE/health" 2>/dev/null || true)"
if [[ -z "$health" ]]; then
    # A 503 body is still worth showing: it says WHICH of the two unhealthy
    # conditions (unfunded, or RPC unreachable) is the problem.
    health="$(curl -sS "$BASE/health" || true)"
    echo "${health:-(no answer — is 'make faucet-poc-run' running?)}"
    echo
    echo "The faucet is not healthy. It will still answer /challenge, but a drip" >&2
    echo "needs a funded key and a reachable RPC." >&2
    exit 1
fi
echo "$health"

say "2/4  Ask for a puzzle.  GET $BASE/challenge?address=$ADDRESS"
challenge="$(curl -fsS --get "$BASE/challenge" --data-urlencode "address=$ADDRESS")"
echo "$challenge"

seed="$(printf '%s' "$challenge" | json_get seed)"
bits="$(printf '%s' "$challenge" | json_get difficulty_bits)"

say "3/4  Solve it ($bits leading zero bits — the same solver the app uses)"
solution="$(cargo run --release --quiet -p eth-faucet -- --solve "$seed" "$bits")"
echo "solution: $solution"

say "4/4  Claim.  POST $BASE/drip"
# The solution rides as a STRING: it is a u128, and JSON numbers are not
# reliably that wide.
body="$(python3 -c '
import json, sys
print(json.dumps({"address": sys.argv[1], "pow_solution": sys.argv[2]}))
' "$ADDRESS" "$solution")"

http_status=0
response="$(curl -sS -o /dev/stdout -w '\n%{http_code}' \
    -H 'Content-Type: application/json' \
    -d "$body" "$BASE/drip")" || true
http_status="$(printf '%s' "$response" | tail -n1)"
payload="$(printf '%s' "$response" | sed '$d')"
echo "$payload"

if [[ "$http_status" != "200" ]]; then
    say "Refused (HTTP $http_status)"
    echo "That is a working faucet saying no — a cooldown, the daily budget, or"
    echo "an address that already has gas. GET $BASE/stats shows every limit in"
    echo "force and what today has spent."
    exit 1
fi

tx="$(printf '%s' "$payload" | json_get tx_hash)"
say "Sent"
echo "https://sepolia.etherscan.io/tx/$tx"
echo
echo "Try it again immediately: the address cooldown will refuse the second one."
echo "GET $BASE/stats to see the drip counted against today's budget."
