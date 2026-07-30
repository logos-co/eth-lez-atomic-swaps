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
//!     pass    (0)  — invalid tx was loudly rejected       => #640 looks FIXED
//!     red     (10) — invalid tx silently accepted (no-op)  => #640 reproduced
//!     fail    (20) — invalid tx was ACCEPTED *and EXECUTED* => the bare-u128
//!                    encoding now moves funds; the canary's #640 assumptions
//!                    have CHANGED and #640 status must be re-derived
//!     broken  (30) — could not run the experiment (no sequencer, pin mismatch,
//!                    or the setup/control preconditions were not met)

use std::collections::BTreeMap;
use std::time::Duration;

use authenticated_transfer_core::Instruction;
use common::HashType;
use lee::public_transaction::{Message, WitnessSet};
use lee::{AccountId, PrivateKey, PublicKey, PublicTransaction};
use lee_core::program::ProgramId;
use sequencer_service_protocol::LeeTransaction;
use sequencer_service_rpc::{ClientError, RpcClient as _, SequencerClient, SequencerClientBuilder};
use url::Url;

const AT_KEY: &str = "authenticated_transfer";

/// JSON-RPC `InvalidParams`. The sequencer's `send_transaction` returns exactly
/// this code (via `ErrorObjectOwned::owned(ErrorCode::InvalidParams, ...)`) when
/// its `transaction_stateless_check()` rejects a transaction — i.e. a genuine
/// instruction-decoding/validation rejection. See the LEZ repo
/// `lez/sequencer/service/src/service.rs::send_transaction`. This is the ONLY
/// submit-time rejection that would prove #640 fixed: nonce/funding are stateful
/// and never cause a submit rejection, so a stale-nonce/unfunded sender cannot
/// masquerade as this.
const JSONRPC_INVALID_PARAMS: i32 = -32602;

/// Why a `send_transaction` call failed, for the malformed-tx branch.
enum RejectKind {
    /// The sequencer actively rejected the tx at stateless validation
    /// (JSON-RPC `InvalidParams`). The only rejection that counts as "#640 fixed".
    Validation,
    /// A transport/timeout/protocol/other error — the experiment could not be
    /// run cleanly. NOT evidence about #640; the canary must report `broken`.
    Infra,
}

/// Classify a `send_transaction` error. Only a server-side `Call` carrying the
/// `InvalidParams` code is a validation rejection; everything else (transport
/// outage, request timeout, restart, parse error, custom, disconnect, or any
/// other JSON-RPC error code) is infrastructure noise.
fn classify_reject(e: &ClientError) -> RejectKind {
    match e {
        ClientError::Call(obj) if obj.code() == JSONRPC_INVALID_PARAMS => RejectKind::Validation,
        _ => RejectKind::Infra,
    }
}

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

    // Distinct recipients so a silent drop is unambiguous: recipient_valid must
    // gain `amount`; recipient_invalid must stay at zero. Fresh accounts must be
    // Initialize'd under authenticated_transfer before they can receive.
    let (recip_valid_sk, recipient_valid) = keypair(rand::random());
    let (recip_invalid_sk, recipient_invalid) = keypair(rand::random());

    // Optional funding + initialization. The malformed-tx verdict is only
    // trustworthy if the invalid-sender was a viable sender to begin with —
    // otherwise a silent no-op could be blamed on an unfunded/uninitialized
    // account rather than the malformed instruction. Funding a *balance* is a
    // NECESSARY but NOT SUFFICIENT proof of that: a positive vault balance does
    // not prove the account was Initialize'd under authenticated_transfer, and a
    // transfer from an uninitialized sender would no-op for setup reasons, not
    // #640. The SUFFICIENT proof is the same-sender/same-recipient EFFECTFUL
    // control below (section 2b): the exact bad_id -> recipient_invalid path is
    // first shown to move funds with a well-formed typed instruction. Here we
    // only track the cheap balance precheck to bail early if funding failed.
    let mut bad_claim_ok = false;
    let mut bad_init_sent = false;
    if args.funded {
        // Move each sender's debug-genesis vault supply into its spendable account.
        for (label, id, sk) in [
            ("valid-sender", sender_id, &sender_sk),
            ("invalid-sender", bad_id, &bad_sk),
        ] {
            match vault_claim(&client, id, sk, args.amount + 100).await {
                Ok(h) => {
                    let ok = poll_balance_at_least(&client, id, args.amount, 20).await.reached();
                    log(&format!("{label} {id}: vault-claim {h}, funded>= {}: {ok}", args.amount));
                    if id == bad_id {
                        bad_claim_ok = ok;
                    }
                }
                Err(e) => log(&format!("{label} vault claim failed (continuing): {e}")),
            }
        }
        // Initialize every account under authenticated_transfer.
        for (label, id, sk) in [
            ("valid-sender", sender_id, &sender_sk),
            ("recipient-valid", recipient_valid, &recip_valid_sk),
            ("invalid-sender", bad_id, &bad_sk),
            ("recipient-invalid", recipient_invalid, &recip_invalid_sk),
        ] {
            match at_initialize(&client, id, sk).await {
                Ok(h) => {
                    log(&format!("{label} authenticated_transfer Initialize: {h}"));
                    if id == bad_id {
                        bad_init_sent = true;
                    }
                }
                Err(e) => log(&format!("{label} Initialize skipped/err (ok if already init): {e}")),
            }
        }
        // Let the Initialize/claim txs land in a block before transferring.
        tokio::time::sleep(Duration::from_secs(18)).await;

        // PRECHECK (necessary, not sufficient): the invalid-sender must at least
        // hold a spendable balance now. This only rules out "funding never
        // landed"; it does NOT prove Initialize succeeded — that is established
        // by the effectful same-sender control (section 2b). Bail early as broken
        // if even this cheap precheck fails.
        let bad_balance_ready = poll_balance_at_least(&client, bad_id, args.amount, 15).await.reached();
        log(&format!(
            "invalid-sender precheck: claim_funded={bad_claim_ok} initialize_sent={bad_init_sent} \
             balance_ready={bad_balance_ready}"
        ));
        if !(bad_claim_ok && bad_balance_ready) {
            verdict(
                "broken",
                &format!(
                    "PRECHECK FAILED: could not even fund the invalid-sender ({bad_id}) before the \
                     #640 experiment (claim_funded={bad_claim_ok}, balance_ready={bad_balance_ready}). \
                     Without spendable funds the same-sender control cannot run, so a malformed no-op \
                     could not be attributed to the instruction vs an unfunded account — not running."
                ),
            );
        }
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

    // GATE on the control BEFORE testing the malformed tx. The malformed-tx
    // verdict is only meaningful if the sequencer is demonstrably including AND
    // executing well-formed transfers on this substrate. If the control does not
    // land, the experiment is confounded — report broken and stop.
    let valid_included = poll_included(&client, valid_h, 15).await;
    if !valid_included {
        verdict(
            "broken",
            &format!(
                "CONTROL FAILED: the valid typed transfer {valid_hash} was accepted at submit but \
                 never included within the poll window — the sequencer is not including transactions, \
                 so the #640 experiment cannot be trusted. Not testing the malformed tx."
            ),
        );
    }
    let valid_effect =
        args.funded && poll_balance_at_least(&client, recipient_valid, args.amount, 30).await.reached();
    if args.funded && !valid_effect {
        verdict(
            "broken",
            &format!(
                "CONTROL FAILED: the valid typed transfer {valid_hash} was included but moved NO \
                 balance to recipient_valid ({recipient_valid}) within the poll window — even a \
                 well-formed transfer did not execute here, so a malformed-tx no-op would be \
                 indistinguishable from a broken substrate. Not testing the malformed tx."
            ),
        );
    }
    let effect_note = if args.funded {
        format!(
            "; CONTROL valid transfer moved {}→recipient (balance effect observed={valid_effect})",
            args.amount
        )
    } else {
        String::new()
    };

    // The session's control-inclusion fact, plumbed into `classify_malformed`
    // to disambiguate a malformed pre-inclusion silent drop (control healthy)
    // from a sequencer stall (control also delayed). Both gates above/below
    // abort as `broken` when their control fails, so this is `true` on every
    // path that reaches the malformed experiment — but the classifier takes it
    // explicitly so the disambiguation is honest and unit-testable.
    let mut control_included = valid_included;

    // 2b. SAME-SENDER, SAME-RECIPIENT EFFECTFUL control --------------------
    // This is the airtight setup proof (P1-1). The malformed-tx verdict must be
    // attributable to the INSTRUCTION ENCODING, not to setup (e.g. the exact
    // sender/recipient not being Initialize'd, which would make ANY transfer a
    // no-op). So — in funded mode — first prove the EXACT bad_id -> recipient_invalid
    // pair used for the malformed tx MOVES FUNDS with a well-formed typed
    // instruction. If it does, the setup question is closed: those two accounts
    // demonstrably work, so a subsequent malformed no-op can only be the encoding.
    // `malformed_baseline` records recipient_invalid's balance AFTER this control
    // so section 3 measures a FURTHER increase (executed) vs no change (silent
    // #640 drop). Only funded mode can observe effects; unfunded stays weaker.
    let mut malformed_baseline: u128 = 0;
    if args.funded {
        let cn = nonces_for(&client, bad_id).await;
        let ctrl_tx = build_transfer(
            at_program,
            bad_id,
            &bad_sk,
            recipient_invalid,
            cn,
            Instruction::Transfer { amount: args.amount },
        );
        let ctrl_h: HashType = match client.send_transaction(ctrl_tx).await {
            Ok(h) => h,
            Err(e) => verdict(
                "broken",
                &format!(
                    "SAME-SENDER CONTROL FAILED: a well-formed typed transfer from the malformed \
                     sender ({bad_id}) to the malformed recipient ({recipient_invalid}) was rejected \
                     at submit ({e}) — cannot establish that this exact path executes typed \
                     instructions, so a malformed no-op would be unattributable. Not testing #640."
                ),
            ),
        };
        let ctrl_hash = ctrl_h.to_string();
        let ctrl_included = poll_included(&client, ctrl_h, 15).await;
        // The recipient must actually GAIN the amount — that is the proof the
        // exact pair is Initialize'd and executes typed transfers.
        let ctrl_effect = poll_balance_at_least(&client, recipient_invalid, args.amount, 30).await.reached();
        if !(ctrl_included && ctrl_effect) {
            verdict(
                "broken",
                &format!(
                    "SAME-SENDER CONTROL FAILED: typed transfer {ctrl_hash} from the malformed sender \
                     ({bad_id}) to the malformed recipient ({recipient_invalid}) did not take effect \
                     (included={ctrl_included}, balance_effect={ctrl_effect}) — this exact path does \
                     not execute even well-formed transfers (likely an uninitialized account), so a \
                     malformed no-op would be indistinguishable from broken setup. Not testing #640."
                ),
            );
        }
        control_included = control_included && ctrl_included;
        // Record the post-control baseline so a malformed EXECUTION would push
        // recipient_invalid strictly above it (baseline + amount).
        malformed_baseline = get_balance(&client, recipient_invalid).await.unwrap_or(args.amount);
        log(&format!(
            "same-sender control OK: {ctrl_hash} moved {}→recipient_invalid \
             (included={ctrl_included}); baseline now {malformed_baseline}",
            args.amount
        ));
    }

    // 3. DELIBERATELY-INVALID bare-u128 transfer (bug #640) ----------------
    let bn = nonces_for(&client, bad_id).await;
    let bad_amount: u128 = args.amount; // bare u128 instead of Instruction::Transfer
    let bad_tx = build_transfer(at_program, bad_id, &bad_sk, recipient_invalid, bn, bad_amount);

    match client.send_transaction(bad_tx).await {
        // A rejection at submit. Only a genuine instruction-decoding/validation
        // rejection (JSON-RPC InvalidParams from the sequencer's stateless
        // check) proves the sequencer LOUDLY rejected the malformed instruction
        // => #640 fixed => PASS. A transport/timeout/other error tells us
        // nothing about #640 (and could even look like a "rejection") => BROKEN.
        Err(e) => match classify_reject(&e) {
            RejectKind::Validation => verdict(
                "pass",
                &format!(
                    "invalid bare-u128 transfer was LOUDLY REJECTED at submit by the sequencer's \
                     validation (JSON-RPC InvalidParams): {e}. #640 appears FIXED. \
                     (control valid tx {valid_hash} included={valid_included}{effect_note})"
                ),
            ),
            RejectKind::Infra => verdict(
                "broken",
                &format!(
                    "malformed transfer submission failed with a NON-validation error \
                     (transport/timeout/protocol, not the sequencer rejecting the instruction): {e}. \
                     This is NOT evidence about #640 — the experiment could not be run cleanly. \
                     (control valid tx {valid_hash} included={valid_included}{effect_note})"
                ),
            ),
        },
        // Accepted at submit: distinguish a silent NO-OP (#640) from actual
        // EXECUTION (P1-2). In funded mode the same-sender control already moved
        // `amount` to recipient_invalid (baseline = malformed_baseline); if the
        // malformed tx also EXECUTES, recipient_invalid rises to at least
        // baseline + amount. If it stays at the baseline, the malformed transfer
        // was a silent no-op despite a demonstrably-working sender => #640.
        Ok(h) => {
            let bad_hash = h.to_string();
            log(&format!("invalid bare-u128 transfer ACCEPTED (no error): {bad_hash}"));
            // Generous window (45×2s = 90s ≈ 6 block intervals). A `red` verdict
            // hinges on confirmed inclusion, so the malformed tx must be given
            // enough block cycles that a false "not-included" (which would
            // suppress a legitimate red) reflects a genuine drop, not impatience.
            let bad_included = poll_included(&client, h, 45).await;
            let want_if_executed = malformed_baseline.saturating_add(args.amount);

            // Only funded mode observes balance effects; polling an unfunded,
            // uninitialized recipient would just burn the window. In unfunded
            // mode the verdict rests on acceptance + inclusion alone (weaker, by
            // design), so `bad_poll` is a sentinel that `classify_malformed`
            // ignores when `funded == false`.
            let bad_poll = if args.funded {
                poll_balance_at_least(&client, recipient_invalid, want_if_executed, 15).await
            } else {
                BalancePoll::BelowThreshold
            };

            // CONCLUSIVENESS GATE (P1): a `red` verdict asserts #640 reproduced.
            // It is trustworthy in exactly two shapes: (a) confirmed INCLUSION
            // plus (funded) a successful balance read showing the recipient
            // stayed at baseline (included-but-no-op), or (b) NEVER INCLUDED
            // while the same session's controls included healthily and reads
            // worked (silently dropped PRE-inclusion — a stall would delay the
            // control too; an outage would fail the reads). Anything else is
            // `broken`, never a false `red`.
            match classify_malformed(args.funded, bad_included, control_included, bad_poll) {
                MalformedVerdict::NotIncluded => verdict(
                    "broken",
                    &format!(
                        "INCONCLUSIVE: the malformed bare-u128 transfer (hash={bad_hash}) was \
                         ACCEPTED at submit but was NEVER INCLUDED within the poll window, and the \
                         session's control was also delayed/not-included (control_included=\
                         {control_included}) — indistinguishable from a sequencer stall/backlog, \
                         so this is NOT evidence #640 reproduced. \
                         (control valid tx {valid_hash} included={valid_included}{effect_note})"
                    ),
                ),
                MalformedVerdict::BalanceUnobservable => verdict(
                    "broken",
                    &format!(
                        "INCONCLUSIVE: the malformed bare-u128 transfer (hash={bad_hash}, \
                         included={bad_included}) was accepted, but EVERY post-submit balance read \
                         of recipient_invalid ({recipient_invalid}) FAILED (RPC unobservable) — \
                         cannot confirm the recipient stayed at baseline {malformed_baseline}, so a \
                         silent no-op cannot be distinguished from an RPC outage. NOT evidence #640 \
                         reproduced. \
                         (control valid tx {valid_hash} included={valid_included}{effect_note})"
                    ),
                ),
                // Accepted + EFFECT observed => the encoding now executes. This
                // breaks the canary's core #640 assumption (silent no-op). It is
                // NOT the expected red light and NOT a pass — a distinct fail(20).
                MalformedVerdict::Executed => verdict(
                    "fail",
                    &format!(
                        "UNEXPECTED-EXECUTION: the sequencer ACCEPTED *and EXECUTED* the malformed \
                         bare-u128 transfer (hash={bad_hash}, included={bad_included}): \
                         recipient_invalid ({recipient_invalid}) balance rose from the same-sender \
                         control baseline {malformed_baseline} to at least {want_if_executed} — the \
                         bare-u128 encoding now MOVES FUNDS. The canary's #640 assumptions have \
                         CHANGED; #640 status is UNKNOWN and must be re-derived. \
                         (same-sender control first moved {}→recipient_invalid; general control \
                         {valid_hash} included={valid_included}{effect_note})",
                        args.amount
                    ),
                ),
                // Accepted, INCLUDED, and (funded) a successful read showed the
                // recipient stayed at baseline: the silent no-op at the heart of #640.
                MalformedVerdict::SilentNoOp => {
                    let drop_note = if args.funded {
                        format!(
                            "; despite a FUNDED, DEMONSTRABLY-WORKING sender (same-sender control \
                             first moved {}→recipient_invalid to baseline {malformed_baseline}) the \
                             malformed transfer moved NO further balance — a successful balance read \
                             showed recipient stayed below {want_if_executed}, as expected",
                            args.amount
                        )
                    } else {
                        "; effect UNOBSERVED (unfunded mode: proves acceptance-with-inclusion and \
                         no error only, not the no-op)"
                            .to_string()
                    };
                    verdict(
                        "red",
                        &format!(
                            "#640 reproduced: sequencer ACCEPTED the malformed bare-u128 transfer \
                             (hash={bad_hash}, included={bad_included}) with no submit error and no \
                             execution error surfaced — a deliberately-invalid instruction was NOT \
                             loudly rejected{drop_note}. See logos-blockchain/logos-execution-zone#640. \
                             (general control: valid typed transfer {valid_hash} included={valid_included}{effect_note})"
                        ),
                    )
                }
                // Accepted at submit, then NEVER INCLUDED — while this same
                // session's well-formed controls included promptly and (funded)
                // balance reads worked and showed no effect. Not a stall (that
                // would delay the control too), not an outage (reads worked):
                // this IS #640's silent drop, one stage earlier than the
                // included-no-op shape.
                MalformedVerdict::SilentDropPreInclusion => {
                    let reads_note = if args.funded {
                        format!(
                            "; balance reads were WORKING and recipient_invalid stayed at the \
                             same-sender control baseline {malformed_baseline} (below \
                             {want_if_executed})"
                        )
                    } else {
                        "; effect UNOBSERVED (unfunded mode: acceptance-then-drop only)".to_string()
                    };
                    verdict(
                        "red",
                        &format!(
                            "#640 reproduced: sequencer ACCEPTED the malformed bare-u128 transfer \
                             (hash={bad_hash}) with no error and then silently dropped pre-inclusion \
                             (control included normally): it was NEVER INCLUDED within the generous \
                             poll window while the session's well-formed control transfers included \
                             promptly (control_included={control_included}){reads_note} — a \
                             deliberately-invalid instruction was NOT loudly rejected. \
                             See logos-blockchain/logos-execution-zone#640. \
                             (general control: valid typed transfer {valid_hash} included={valid_included}{effect_note})"
                        ),
                    )
                }
            }
        }
    }
}

/// Single balance read (no polling); `None` on RPC error.
async fn get_balance(client: &SequencerClient, id: AccountId) -> Option<u128> {
    client.get_account_balance(id).await.ok()
}

/// Poll get_transaction until the hash resolves (included) or we give up.
/// `tries` polls × 2s each. The debug localnet's `block_create_timeout` is 15s,
/// so a window must span several block intervals to reliably observe inclusion —
/// especially for the malformed tx, whose false "not-included" would suppress a
/// legitimate `red` (see the malformed branch, which uses a generous window).
async fn poll_included(client: &SequencerClient, hash: HashType, tries: u32) -> bool {
    for _ in 0..tries {
        if let Ok(Some(_)) = client.get_transaction(hash).await {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

/// Outcome of polling an account balance against a threshold. Unlike a bare
/// bool, this DISTINGUISHES "the reads succeeded and the balance stayed below
/// the threshold" (a trustworthy observation) from "every read failed" (the
/// balance is unobservable — no evidence either way). That distinction is what
/// lets the malformed-tx branch avoid a false `red` during an RPC outage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BalancePoll {
    /// At least one read succeeded AND the balance reached `want`.
    Reached,
    /// At least one read succeeded, but the balance never reached `want` within
    /// the window. A TRUSTWORTHY "stayed below threshold" observation.
    BelowThreshold,
    /// Every read failed (RPC error/outage) — the balance is UNOBSERVABLE.
    Unobservable,
}

impl BalancePoll {
    /// True only when the threshold was observed to be reached. Preserves the
    /// old bare-bool contract for the setup/control call sites, which only care
    /// whether the balance climbed to `want`.
    fn reached(self) -> bool {
        matches!(self, BalancePoll::Reached)
    }
}

async fn poll_balance_at_least(
    client: &SequencerClient,
    id: AccountId,
    want: u128,
    tries: u32,
) -> BalancePoll {
    let mut any_read_ok = false;
    for _ in 0..tries {
        if let Ok(bal) = client.get_account_balance(id).await {
            any_read_ok = true;
            if bal >= want {
                return BalancePoll::Reached;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    if any_read_ok {
        BalancePoll::BelowThreshold
    } else {
        BalancePoll::Unobservable
    }
}

/// The terminal verdict for the ACCEPTED-at-submit malformed tx, as a pure,
/// unit-testable value. A `red` (#640 reproduced) verdict is CONCLUSIVE only
/// when EITHER
///   (a) the malformed tx was confirmed INCLUDED and — in funded mode — a
///       balance read actually succeeded and showed the recipient stayed at
///       baseline (the included-but-no-op shape), OR
///   (b) the malformed tx was accepted at submit but NEVER INCLUDED within a
///       generous window while the SAME SESSION's well-formed control txs
///       demonstrably included healthily and (funded) balance reads were
///       working — the silently-dropped-PRE-INCLUSION shape. A sequencer stall
///       would have delayed the control too, and an RPC outage would have made
///       the reads fail, so neither can masquerade as this.
/// Anything short of that is `broken`, not `red`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MalformedVerdict {
    /// broken(30): accepted at submit but never included, AND the session's
    /// control was ALSO delayed/not-included — indistinguishable from a
    /// sequencer stall, so not evidence about #640.
    NotIncluded,
    /// broken(30): every post-submit balance read failed (RPC unobservable) —
    /// no conclusive funded-mode verdict is possible, included or not.
    BalanceUnobservable,
    /// fail(20): the recipient balance rose — the encoding executes.
    Executed,
    /// red(10): included, and (funded) a read showed the recipient stayed at
    /// baseline — the silent no-op at the heart of #640.
    SilentNoOp,
    /// red(10): never included within the generous window while the session's
    /// control included normally (and, funded, reads worked and showed no
    /// effect) — #640's silent drop, one stage earlier: dropped PRE-inclusion.
    SilentDropPreInclusion,
}

/// Classify the accepted-at-submit malformed tx. Pure so it is unit-testable.
/// `funded` selects whether a balance observation is required for a conclusive
/// verdict; in unfunded mode `bad_poll` is a sentinel (no balances observed).
/// `control_included` is the session's control-inclusion fact (the general
/// valid control AND, in funded mode, the same-sender control) — it
/// disambiguates a pre-inclusion silent drop (control healthy => red) from a
/// sequencer stall (control also delayed => broken).
fn classify_malformed(
    funded: bool,
    bad_included: bool,
    control_included: bool,
    bad_poll: BalancePoll,
) -> MalformedVerdict {
    if funded {
        match bad_poll {
            // Observed effect is ground truth: the encoding moved funds,
            // whether or not get_transaction resolved the hash.
            BalancePoll::Reached => return MalformedVerdict::Executed,
            // Every read failed: nothing conclusive can be said in funded mode.
            BalancePoll::Unobservable => return MalformedVerdict::BalanceUnobservable,
            // Reads succeeded and the recipient stayed at baseline.
            BalancePoll::BelowThreshold => {}
        }
    }
    if bad_included {
        return MalformedVerdict::SilentNoOp;
    }
    if control_included {
        return MalformedVerdict::SilentDropPreInclusion;
    }
    MalformedVerdict::NotIncluded
}

#[cfg(test)]
mod tests {
    use super::{classify_malformed, BalancePoll, MalformedVerdict};

    // Shape (a): included + funded + a successful read showing the recipient
    // stayed at baseline => red(10) (the included-but-no-op shape).
    #[test]
    fn included_observed_baseline_is_red() {
        assert_eq!(
            classify_malformed(true, true, true, BalancePoll::BelowThreshold),
            MalformedVerdict::SilentNoOp
        );
    }

    // Shape (b): accepted-at-submit + NEVER included + the session's control
    // included healthily + balance reads working => red(10), the pre-inclusion
    // silent drop (this is how #640 manifests on the real v0.2.0 localnet).
    #[test]
    fn not_included_control_healthy_is_red_pre_inclusion_drop() {
        assert_eq!(
            classify_malformed(true, false, true, BalancePoll::BelowThreshold),
            MalformedVerdict::SilentDropPreInclusion
        );
        // Unfunded mode: no balances observed (sentinel BelowThreshold); the
        // acceptance-then-drop with a healthy control is still the red shape.
        assert_eq!(
            classify_malformed(false, false, true, BalancePoll::BelowThreshold),
            MalformedVerdict::SilentDropPreInclusion
        );
    }

    // Stall disambiguation: never included AND the control was ALSO delayed/
    // not-included => broken(30) — a stall, not evidence about #640.
    #[test]
    fn both_not_included_is_broken_stall() {
        for funded in [true, false] {
            assert_eq!(
                classify_malformed(funded, false, false, BalancePoll::BelowThreshold),
                MalformedVerdict::NotIncluded,
                "funded={funded}"
            );
        }
    }

    // Funded, every balance read failed => broken(30), NOT a false red —
    // regardless of inclusion or control health.
    #[test]
    fn reads_all_failed_is_broken() {
        for (bad_included, control_included) in [(true, true), (false, true), (false, false)] {
            assert_eq!(
                classify_malformed(true, bad_included, control_included, BalancePoll::Unobservable),
                MalformedVerdict::BalanceUnobservable,
                "bad_included={bad_included} control_included={control_included}"
            );
        }
    }

    // Funded + recipient balance rose => fail(20): the encoding now executes.
    // The observed effect is ground truth even if inclusion wasn't resolved.
    #[test]
    fn included_observed_increase_is_fail() {
        assert_eq!(
            classify_malformed(true, true, true, BalancePoll::Reached),
            MalformedVerdict::Executed
        );
        assert_eq!(
            classify_malformed(true, false, true, BalancePoll::Reached),
            MalformedVerdict::Executed
        );
    }

    // Unfunded mode: no balances observed, so an included accept => red(10) on
    // acceptance+inclusion alone (weaker, by design); poll is ignored.
    #[test]
    fn unfunded_included_is_red_ignoring_poll() {
        for poll in [
            BalancePoll::Reached,
            BalancePoll::BelowThreshold,
            BalancePoll::Unobservable,
        ] {
            assert_eq!(
                classify_malformed(false, true, true, poll),
                MalformedVerdict::SilentNoOp,
                "poll={poll:?}"
            );
        }
    }
}
