use std::io::Write;

use tokio::io::{AsyncBufReadExt, BufReader};
use tracing_subscriber::fmt::MakeWriter;

use crate::demo::DemoEnv;
use crate::error::{Result, SwapError};
use crate::messaging::client::MessagingClient;
use crate::messaging::types::OFFERS_TOPIC;

const NWAKU_URL: &str = "http://localhost:8645";

// ── Color-coded log prefixes ───────────────────────────────────────

const ANVIL_PREFIX: &str = "  \x1b[33m[anvil]\x1b[0m ";    // yellow
const LEZ_PREFIX: &str = "  \x1b[36m[lez]\x1b[0m   ";      // cyan

// ── Custom tracing writer that prefixes each log line ──────────────

/// Buffers all writes for a single tracing event, then flushes with a
/// colored prefix on drop.
struct PrefixedWriter {
    buf: Vec<u8>,
    prefix: &'static [u8],
}

impl std::io::Write for PrefixedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for PrefixedWriter {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(self.prefix);
            let _ = stderr.write_all(&self.buf);
            let _ = stderr.flush();
        }
    }
}

struct PrefixedWriterFactory {
    prefix: &'static [u8],
}

impl<'a> MakeWriter<'a> for PrefixedWriterFactory {
    type Writer = PrefixedWriter;
    fn make_writer(&'a self) -> Self::Writer {
        PrefixedWriter {
            buf: Vec::with_capacity(256),
            prefix: self.prefix,
        }
    }
}

// ── Anvil stdout forwarder ─────────────────────────────────────────

fn spawn_anvil_log_forwarder(stdout: std::process::ChildStdout) {
    let stdout = tokio::process::ChildStdout::from_std(stdout).unwrap();
    tokio::spawn(async move {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("{ANVIL_PREFIX}{line}");
        }
    });
}

// ── Main command ───────────────────────────────────────────────────

pub async fn cmd_infra() -> Result<()> {
    // Set up tracing with [lez] prefix for sequencer logs.
    tracing_subscriber::fmt()
        .with_writer(PrefixedWriterFactory {
            prefix: LEZ_PREFIX.as_bytes(),
        })
        .with_ansi(true)
        .init();

    println!();
    println!("\x1b[1m=== Atomic Swap Infrastructure ===\x1b[0m");
    println!();

    // 1. Check nwaku health.
    eprint!("  [\x1b[35mnwaku\x1b[0m] Checking {}...", NWAKU_URL);
    let messaging = MessagingClient::new(NWAKU_URL);
    messaging.subscribe(&[OFFERS_TOPIC]).await.map_err(|_| {
        SwapError::Messaging(format!(
            "cannot reach nwaku at {NWAKU_URL} — run `make nwaku` first"
        ))
    })?;
    eprintln!(" \x1b[35mOK\x1b[0m");

    // 2. Start DemoEnv (Anvil + LEZ sequencer + deploy contracts).
    let mut env = DemoEnv::start(Some(Box::new(|step, label, detail| {
        let color = match step {
            1 => "\x1b[33m",      // yellow — anvil
            2 => "\x1b[32m",      // green  — deploy
            3 => "\x1b[36m",      // cyan   — LEZ accounts
            4 | 5 => "\x1b[36m",  // cyan   — sequencer / program
            _ => "\x1b[0m",
        };
        let tag = match step {
            1 => "anvil",
            2 => "deploy",
            3 => "lez",
            4 => "sequencer",
            5 => "deploy",
            _ => "infra",
        };
        if detail.is_empty() {
            eprint!("  [{color}{tag}\x1b[0m] {label}...");
        } else {
            eprintln!(" {color}{detail}\x1b[0m");
        }
    })))
    .await;

    // 3. Start forwarding Anvil logs.
    if let Some(stdout) = env.anvil_stdout.take() {
        spawn_anvil_log_forwarder(stdout);
    }

    // 4. Write .env (maker config).
    write_env_file(".env", &env, Role::Maker)?;
    eprintln!("  [\x1b[1minfra\x1b[0m] Wrote .env (maker)");

    // 5. Write .env.taker.
    write_env_file(".env.taker", &env, Role::Taker)?;
    eprintln!("  [\x1b[1minfra\x1b[0m] Wrote .env.taker (taker)");

    // 6. Print summary.
    println!();
    println!("\x1b[1m┌──────────────────────────────────────────────────┐\x1b[0m");
    println!("\x1b[1m│  Infrastructure Ready                            │\x1b[0m");
    println!("\x1b[1m├──────────────────────────────────────────────────┤\x1b[0m");
    println!("│  \x1b[33mAnvil (ETH)\x1b[0m:   {}  │", env.maker_config.eth_rpc_url);
    println!("│  \x1b[32mETH HTLC\x1b[0m:      {}           │", env.maker_config.eth_htlc_address);
    println!("│  \x1b[36mLEZ Sequencer\x1b[0m: {:<33}│", env.maker_config.lez_sequencer_url);
    println!("│  \x1b[35mNwaku\x1b[0m:         {:<33}│", NWAKU_URL);
    println!("│  Maker .env:    {:<33}│", ".env");
    println!("│  Taker .env:    {:<33}│", ".env.taker");
    println!("\x1b[1m└──────────────────────────────────────────────────┘\x1b[0m");
    println!();
    println!("  \x1b[2mLogs: \x1b[33m[anvil]\x1b[0m\x1b[2m  \x1b[36m[lez]\x1b[0m\x1b[2m  \x1b[35m[nwaku]\x1b[0m\x1b[2m → docker compose logs -f\x1b[0m");
    println!("  Press Ctrl-C to stop all services.");
    println!();

    // 7. Block until Ctrl-C.
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| SwapError::InvalidConfig(format!("signal error: {e}")))?;

    println!();
    eprintln!("  [\x1b[1minfra\x1b[0m] Shutting down...");

    // DemoEnv drops here, cleaning up Anvil + sequencer.
    drop(env);

    Ok(())
}

enum Role {
    Maker,
    Taker,
}

fn write_env_file(path: &str, env: &DemoEnv, role: Role) -> Result<()> {
    let config = match role {
        Role::Maker => &env.maker_config,
        Role::Taker => &env.taker_config,
    };

    let program_id_bytes: Vec<u8> = config
        .lez_htlc_program_id
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();

    let contents = format!(
        "\
# Auto-generated by `swap-cli infra` — do not edit while infra is running.

# Ethereum
ETH_RPC_URL={eth_rpc}
ETH_PRIVATE_KEY={eth_key}
ETH_HTLC_ADDRESS={eth_htlc}

# LEZ
LEZ_SEQUENCER_URL={lez_seq}
LEZ_SIGNING_KEY={lez_key}
LEZ_HTLC_PROGRAM_ID={lez_prog}

# Swap parameters
LEZ_AMOUNT={lez_amount}
ETH_AMOUNT={eth_amount}

# Timelocks (taker locks ETH first with longer timelock)
ETH_TIMELOCK_MINUTES=10
LEZ_TIMELOCK_MINUTES=5

# Counterparty
ETH_RECIPIENT_ADDRESS={eth_recipient}
LEZ_TAKER_ACCOUNT_ID={lez_taker}

# Polling
POLL_INTERVAL_MS=500

# Logos Messaging
NWAKU_URL={nwaku}
",
        eth_rpc = config.eth_rpc_url,
        eth_key = config.eth_private_key,
        eth_htlc = config.eth_htlc_address,
        lez_seq = config.lez_sequencer_url,
        lez_key = config.lez_signing_key,
        lez_prog = hex::encode(&program_id_bytes),
        lez_amount = config.lez_amount,
        eth_amount = crate::config::wei_to_eth_string(config.eth_amount),
        eth_recipient = config.eth_recipient_address,
        lez_taker = hex::encode(config.lez_taker_account_id.value()),
        nwaku = NWAKU_URL,
    );

    let mut f = std::fs::File::create(path)
        .map_err(|e| SwapError::InvalidConfig(format!("failed to write {path}: {e}")))?;
    f.write_all(contents.as_bytes())
        .map_err(|e| SwapError::InvalidConfig(format!("failed to write {path}: {e}")))?;

    Ok(())
}
