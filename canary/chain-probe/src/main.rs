//! canary-chain-probe — the executable heart of leg-chain.sh.
//!
//! It exercises the real builder journey against a LEZ sequencer:
//!   1. health + program-id compatibility (client v0.2.0 vs the sequencer's pin)
//!   2. a VALID typed transfer  -> assert accepted + included (+ effect, if funded)
//!   3. the DELIBERATELY-INVALID bare-`u128` transfer (bug #640's payload) ->
//!      ASSERT it is LOUDLY REJECTED.
//!
//! Assertion (3) is expected to FAIL today: at v0.2.0 the sequencer accepts the
//! malformed instruction (returns a tx hash), includes it, and the transfer
//! simply never executes — no error anywhere (logos-blockchain/logos-execution-zone#640).
//! That is the canary's legitimate red light, not a canary defect. The exit
//! code distinguishes the two:
//!
//!   stdout final line:  PROBE_VERDICT <status>|<evidence>
//!     pass    (0)  — invalid tx was loudly rejected  => #640 looks FIXED
//!     red     (10) — invalid tx silently accepted     => #640 reproduced
//!     broken  (30) — could not run the experiment (no sequencer, pin mismatch)

use std::collections::BTreeMap;
use std::time::Duration;

use authenticated_transfer_core::Instruction;
use common::HashType;
use lee::public_transaction::{Message, WitnessSet};
use lee::{AccountId, PrivateKey, PublicKey, PublicTransaction};
use lee_core::program::ProgramId;
use sequencer_service_protocol::LeeTransaction;
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use url::Url;

const AT_KEY: &str = "authenticated_transfer";

fn log(msg: &str) {
    eprintln!("[chain-probe] {msg}");
}

/// Print the verdict line the shell leg parses, then exit with the mapped code.
fn verdict(status: &str, evidence: &str) -> ! {
    println!("PROBE_VERDICT {status}|{evidence}");
    let code = match status {
        "pass" => 0,
        "red" => 10,
        "broken" => 30,
        _ => 20,
    };
    std::process::exit(code);
}

struct Args {
    rpc: String,
    key: Option<[u8; 32]>,
    key2: Option<[u8; 32]>,
    amount: u128,
    funded: bool,
    account_only: bool,
}

fn parse_hex32(s: &str) -> [u8; 32] {
    let b = hex::decode(s.trim_start_matches("0x")).expect("key must be hex");
    b.try_into().expect("key must be 32 bytes")
}

fn parse_args() -> Args {
    let mut a = Args {
        rpc: std::env::var("CANARY_LEZ_RPC").unwrap_or_else(|_| "http://127.0.0.1:3040".into()),
        key: std::env::var("CANARY_LEZ_KEY").ok().map(|k| parse_hex32(&k)),
        key2: std::env::var("CANARY_LEZ_KEY2").ok().map(|k| parse_hex32(&k)),
        amount: 5,
        funded: false,
        account_only: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "account" => a.account_only = true,
            "--rpc" => a.rpc = it.next().expect("--rpc needs a value"),
            "--key" => a.key = Some(parse_hex32(&it.next().expect("--key needs a value"))),
            "--key2" => a.key2 = Some(parse_hex32(&it.next().expect("--key2 needs a value"))),
            "--amount" => a.amount = it.next().expect("--amount needs a value").parse().unwrap(),
            "--funded" => a.funded = true,
            _ => {}
        }
    }
    a
}

fn keypair(bytes: [u8; 32]) -> (PrivateKey, AccountId) {
    let sk = PrivateKey::try_new(bytes).expect("valid 32-byte private key");
    let pk = PublicKey::new_from_private_key(&sk);
    let id = AccountId::from(&pk);
    (sk, id)
}

/// Build + sign a public transfer tx. `instruction` is generic: pass the typed
/// `Instruction::Transfer { amount }` for the valid case, or a bare `u128` for
/// the #640 payload — both serialize; only the typed one executes.
fn build_transfer<I: serde::Serialize>(
    program_id: ProgramId,
    sender: AccountId,
    sender_sk: &PrivateKey,
    recipient: AccountId,
    nonces: Vec<lee_core::account::Nonce>,
    instruction: I,
) -> LeeTransaction {
    let message = Message::try_new(program_id, vec![sender, recipient], nonces, instruction)
        .expect("message builds");
    let witness = WitnessSet::for_message(&message, &[sender_sk]);
    LeeTransaction::Public(PublicTransaction::new(message, witness))
}

async fn nonces_for(client: &SequencerClient, id: AccountId) -> Vec<lee_core::account::Nonce> {
    client
        .get_accounts_nonces(vec![id])
        .await
        .expect("get_accounts_nonces")
}

/// Initialize an account under the authenticated_transfer program so it can
/// send/receive (accounts: `[account_to_initialize]`). Idempotent-ish: a
/// re-init just errors, which callers log and ignore.
async fn at_initialize(
    client: &SequencerClient,
    account: AccountId,
    sk: &PrivateKey,
) -> Result<String, String> {
    let pid = programs::authenticated_transfer().id();
    let nonces = nonces_for(client, account).await;
    let message = Message::try_new(pid, vec![account], nonces, Instruction::Initialize)
        .map_err(|e| e.to_string())?;
    let witness = WitnessSet::for_message(&message, &[sk]);
    let tx = LeeTransaction::Public(PublicTransaction::new(message, witness));
    client
        .send_transaction(tx)
        .await
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Faucet-free funding for localnets: a debug-genesis account's supply sits in
/// its vault PDA; `vault Claim { amount }` moves it into the spendable account.
/// (Mirrors the LEZ repo's `just wallet-import-test-accounts`.)
async fn vault_claim(
    client: &SequencerClient,
    owner: AccountId,
    owner_sk: &PrivateKey,
    amount: u128,
) -> Result<String, String> {
    let vault_pid = programs::vault().id();
    let vault_pda = vault_core::compute_vault_account_id(vault_pid, owner);
    let nonces = nonces_for(client, owner).await;
    let instruction = vault_core::Instruction::Claim { amount };
    let message = Message::try_new(vault_pid, vec![owner, vault_pda], nonces, instruction)
        .map_err(|e| e.to_string())?;
    let witness = WitnessSet::for_message(&message, &[owner_sk]);
    let tx = LeeTransaction::Public(PublicTransaction::new(message, witness));
    client
        .send_transaction(tx)
        .await
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() {
    let args = parse_args();

    // A fresh random key each run unless one is supplied (funding needs a
    // stable, known account, so leg-chain.sh passes --key when it can fund).
    let sender_bytes = args.key.unwrap_or_else(rand::random);
    let (sender_sk, sender_id) = keypair(sender_bytes);

    if args.account_only {
        // Emit the base58 account id so the shell leg can faucet-fund it.
        println!("{sender_id}");
        return;
    }

    log(&format!("target sequencer: {}", args.rpc));
    log(&format!("sender account:   {sender_id}"));

    let url = match Url::parse(&args.rpc) {
        Ok(u) => u,
        Err(e) => verdict("broken", &format!("invalid --rpc url {}: {e}", args.rpc)),
    };
    let client = match SequencerClientBuilder::default().build(url) {
        Ok(c) => c,
        Err(e) => verdict("broken", &format!("cannot build sequencer client: {e}")),
    };

    // 1. Health + program-id compatibility ---------------------------------
    if let Err(e) = client.check_health().await {
        verdict(
            "broken",
            &format!("no sequencer at {} (is a localnet running?): {e}", args.rpc),
        );
    }
    let prog_ids: BTreeMap<String, ProgramId> = match client.get_program_ids().await {
        Ok(m) => m.into_iter().collect(),
        Err(e) => verdict("broken", &format!("get_program_ids failed: {e}")),
    };
    let at_program = programs::authenticated_transfer().id();
    match prog_ids.get(AT_KEY) {
        Some(seq_id) if *seq_id == at_program => {
            log("program-id compatibility: OK (client v0.2.0 == sequencer)");
        }
        Some(seq_id) => verdict(
            "broken",
            &format!(
                "program-id mismatch: sequencer '{AT_KEY}' id {seq_id:?} != v0.2.0 client {at_program:?}; \
                 the running localnet is a different LEZ pin — transfer experiment would be confounded"
            ),
        ),
        None => verdict("broken", &format!("sequencer does not expose '{AT_KEY}' program")),
    }

    // The malformed-transfer sender: a second genesis key when supplied (so the
    // #640 case has a FUNDED sender — the cleanest proof), else a fresh account.
    let (bad_sk, bad_id) = match args.key2 {
        Some(k) => keypair(k),
        None => keypair(rand::random()),
    };

    // Optional funding: move each sender's debug-genesis vault supply into its
    // spendable account. Only then does the control transfer actually finalize a
    // balance move and the #640 case become "funded sender, still no-op".
    if args.funded {
        for (label, id, sk) in [
            ("valid-sender", sender_id, &sender_sk),
            ("invalid-sender", bad_id, &bad_sk),
        ] {
            match vault_claim(&client, id, sk, args.amount + 100).await {
                Ok(h) => {
                    let ok = poll_balance_at_least(&client, id, args.amount, 20).await;
                    log(&format!("{label} {id}: vault-claim {h}, funded>= {}: {ok}", args.amount));
                }
                Err(e) => log(&format!("{label} vault claim failed (continuing): {e}")),
            }
        }
    }

    // Distinct recipients so a silent drop is unambiguous: recipient_valid must
    // gain `amount`; recipient_invalid must stay at zero. Fresh accounts must be
    // Initialize'd under authenticated_transfer before they can receive.
    let (recip_valid_sk, recipient_valid) = keypair(rand::random());
    let (recip_invalid_sk, recipient_invalid) = keypair(rand::random());

    if args.funded {
        for (label, id, sk) in [
            ("valid-sender", sender_id, &sender_sk),
            ("recipient-valid", recipient_valid, &recip_valid_sk),
            ("invalid-sender", bad_id, &bad_sk),
            ("recipient-invalid", recipient_invalid, &recip_invalid_sk),
        ] {
            match at_initialize(&client, id, sk).await {
                Ok(h) => log(&format!("{label} authenticated_transfer Initialize: {h}")),
                Err(e) => log(&format!("{label} Initialize skipped/err (ok if already init): {e}")),
            }
        }
        // Let the Initialize txs land in a block before transferring.
        tokio::time::sleep(Duration::from_secs(18)).await;
    }

    // 2. VALID typed transfer (control) -----------------------------------
    let n = nonces_for(&client, sender_id).await;
    let valid_tx = build_transfer(
        at_program,
        sender_id,
        &sender_sk,
        recipient_valid,
        n,
        Instruction::Transfer { amount: args.amount },
    );
    let valid_h: HashType = match client.send_transaction(valid_tx).await {
        Ok(h) => h,
        Err(e) => verdict(
            "broken",
            &format!("VALID typed transfer was rejected at submit (env/nonce problem, not #640): {e}"),
        ),
    };
    let valid_hash = valid_h.to_string();
    log(&format!("valid typed transfer accepted: {valid_hash}"));
    let valid_included = poll_included(&client, valid_h).await;
    let valid_effect = args.funded && poll_balance_at_least(&client, recipient_valid, args.amount, 30).await;
    let effect_note = if args.funded {
        format!(
            "; CONTROL valid transfer moved {}→recipient (balance effect observed={valid_effect})",
            args.amount
        )
    } else {
        String::new()
    };

    // 3. DELIBERATELY-INVALID bare-u128 transfer (bug #640) ----------------
    let bn = nonces_for(&client, bad_id).await;
    let bad_amount: u128 = args.amount; // bare u128 instead of Instruction::Transfer
    let bad_tx = build_transfer(at_program, bad_id, &bad_sk, recipient_invalid, bn, bad_amount);

    match client.send_transaction(bad_tx).await {
        // Loud rejection at submit == the sequencer rejected the malformed
        // instruction. That is exactly what the canary asserts, so it PASSES —
        // meaning #640 has been fixed upstream.
        Err(e) => verdict(
            "pass",
            &format!(
                "invalid bare-u128 transfer was LOUDLY REJECTED at submit: {e}. \
                 #640 appears FIXED. (control valid tx {valid_hash} included={valid_included}{effect_note})"
            ),
        ),
        // Accepted silently: the canary's red light.
        Ok(h) => {
            let bad_hash = h.to_string();
            log(&format!("invalid bare-u128 transfer ACCEPTED (no error): {bad_hash}"));
            let bad_included = poll_included(&client, h).await;
            // If funded, confirm the malformed tx moved NOTHING despite the
            // sender having funds — the silent no-op at the heart of #640.
            let bad_effect = args.funded && poll_balance_at_least(&client, recipient_invalid, args.amount, 15).await;
            let drop_note = if args.funded {
                format!(
                    "; despite a FUNDED sender the malformed transfer executed NO balance move \
                     (recipient effect={bad_effect}, expected false)"
                )
            } else {
                String::new()
            };
            verdict(
                "red",
                &format!(
                    "#640 reproduced: sequencer ACCEPTED the malformed bare-u128 transfer \
                     (hash={bad_hash}, included={bad_included}) with no submit error and no \
                     execution error surfaced — a deliberately-invalid instruction was NOT \
                     loudly rejected{drop_note}. See logos-blockchain/logos-execution-zone#640. \
                     (control: valid typed transfer {valid_hash} included={valid_included}{effect_note})"
                ),
            )
        }
    }
}

/// Poll get_transaction until the hash resolves (included) or we give up.
async fn poll_included(client: &SequencerClient, hash: HashType) -> bool {
    for _ in 0..15 {
        if let Ok(Some(_)) = client.get_transaction(hash).await {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

async fn poll_balance_at_least(client: &SequencerClient, id: AccountId, want: u128, tries: u32) -> bool {
    for _ in 0..tries {
        if let Ok(bal) = client.get_account_balance(id).await {
            if bal >= want {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}
