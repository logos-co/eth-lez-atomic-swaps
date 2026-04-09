use std::io::Write;
use std::time::Duration;

use alloy::primitives::U256;
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use lez_htlc_methods::{LEZ_HTLC_PROGRAM_ELF, LEZ_HTLC_PROGRAM_ID};
use common::transaction::NSSATransaction;
use nssa::{
    ProgramDeploymentTransaction,
    program_deployment_transaction::Message as ProgramDeploymentMessage,
};
use sequencer_service_rpc::RpcClient as _;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::config::LezAuth;
use crate::error::{Result, SwapError};
use crate::scaffold;

sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

const BLOCK_WAIT: Duration = Duration::from_secs(4);

// ── Color-coded log prefixes ───────────────────────────────────────

const ANVIL_PREFIX: &str = "  \x1b[33m[anvil]\x1b[0m ";    // yellow

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
    tracing_subscriber::fmt::init();

    println!();
    println!("\x1b[1m=== Atomic Swap Infrastructure ===\x1b[0m");
    println!();

    // 1. Read scaffold wallet via WalletCore.
    eprint!("  [\x1b[36mlez\x1b[0m]   Reading scaffold wallet...");
    let wc = scaffold::wallet_core(&scaffold::wallet_home())?;
    let accounts = scaffold::public_accounts(&wc)?;
    let wallet_home = scaffold::wallet_home();
    let sequencer_url = scaffold::sequencer_url_of(&wc);
    eprintln!(
        " \x1b[36mmaker={} taker={}\x1b[0m",
        &accounts[0].account_id_b58[..8],
        &accounts[1].account_id_b58[..8]
    );

    // 3. Fund accounts.
    eprint!("  [\x1b[36mlez\x1b[0m]   Funding accounts...");
    scaffold::wallet_topup(Some(&accounts[0].account_id_b58)).await?;
    scaffold::wallet_topup(Some(&accounts[1].account_id_b58)).await?;
    eprintln!(" \x1b[36mOK\x1b[0m");

    // 4. Start Anvil.
    eprint!("  [\x1b[33manvil\x1b[0m] Starting Anvil...");
    let mut anvil = alloy::node_bindings::Anvil::new()
        .block_time(1)
        .arg("--balance").arg("100")
        .keep_stdout()
        .try_spawn()
        .unwrap();
    let anvil_ws = anvil.ws_endpoint();
    let anvil_stdout = anvil.child_mut().stdout.take();
    eprintln!(" \x1b[33m{}\x1b[0m", &anvil_ws);

    // 5. Deploy EthHTLC contract.
    eprint!("  [\x1b[32mdeploy\x1b[0m] Deploying EthHTLC...");
    let maker_eth_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let maker_eth_addr = maker_eth_signer.address();

    let deployer = ProviderBuilder::new()
        .wallet(maker_eth_signer)
        .connect_ws(WsConnect::new(&anvil_ws))
        .await
        .unwrap()
        .erased();
    let contract = EthHTLC::deploy(&deployer, U256::from(60u64))
        .await
        .unwrap();
    let eth_htlc_address = *contract.address();
    eprintln!(" \x1b[32m{}\x1b[0m", eth_htlc_address);

    // 6. Deploy LEZ HTLC program.
    eprint!("  [\x1b[32mdeploy\x1b[0m] Deploying LEZ HTLC program...");
    let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
    let tx = ProgramDeploymentTransaction { message: msg };
    wc.sequencer_client
        .send_transaction(NSSATransaction::ProgramDeployment(tx))
        .await
        .unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;
    eprintln!(" \x1b[32mdeployed\x1b[0m");

    // 7. Start forwarding Anvil logs.
    if let Some(stdout) = anvil_stdout {
        spawn_anvil_log_forwarder(stdout);
    }

    // 8. Write .env files.
    let program_id_bytes: Vec<u8> = LEZ_HTLC_PROGRAM_ID
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let program_id_hex = hex::encode(&program_id_bytes);
    let wallet_home_abs = std::fs::canonicalize(&wallet_home)
        .unwrap_or_else(|_| wallet_home.clone());
    let wallet_home_str = wallet_home_abs.display().to_string();

    write_env_file(
        ".env",
        &EnvParams {
            eth_rpc_url: &anvil_ws,
            eth_private_key: &hex::encode(anvil.keys()[0].to_bytes()),
            eth_htlc_address: &format!("{eth_htlc_address}"),
            lez_sequencer_url: &sequencer_url,
            lez_auth: &LezAuth::Wallet {
                home: wallet_home.clone(),
                account_id: accounts[0].account_id,
            },
            lez_htlc_program_id: &program_id_hex,
            lez_amount: 10,
            eth_amount: "10",
            eth_recipient: &format!("{maker_eth_addr}"),
            lez_taker_account: &accounts[1].account_id_b58,
            nssa_wallet_home_dir: &wallet_home_str,
        },
    )?;
    eprintln!("  [\x1b[1minfra\x1b[0m] Wrote .env (maker)");

    write_env_file(
        ".env.taker",
        &EnvParams {
            eth_rpc_url: &anvil_ws,
            eth_private_key: &hex::encode(anvil.keys()[1].to_bytes()),
            eth_htlc_address: &format!("{eth_htlc_address}"),
            lez_sequencer_url: &sequencer_url,
            lez_auth: &LezAuth::Wallet {
                home: wallet_home,
                account_id: accounts[1].account_id,
            },
            lez_htlc_program_id: &program_id_hex,
            lez_amount: 10,
            eth_amount: "10",
            eth_recipient: &format!("{maker_eth_addr}"),
            lez_taker_account: &accounts[1].account_id_b58,
            nssa_wallet_home_dir: &wallet_home_str,
        },
    )?;
    eprintln!("  [\x1b[1minfra\x1b[0m] Wrote .env.taker (taker)");

    // 9. Print summary.
    println!();
    println!("\x1b[1m┌──────────────────────────────────────────────────┐\x1b[0m");
    println!("\x1b[1m│  Infrastructure Ready                            │\x1b[0m");
    println!("\x1b[1m├──────────────────────────────────────────────────┤\x1b[0m");
    println!("│  \x1b[33mAnvil (ETH)\x1b[0m:   {:<33}│", &anvil_ws);
    println!("│  \x1b[32mETH HTLC\x1b[0m:      {}           │", eth_htlc_address);
    println!("│  \x1b[36mLEZ Sequencer\x1b[0m: {:<33}│", &sequencer_url);
    println!("│  Maker .env:    {:<33}│", ".env");
    println!("│  Taker .env:    {:<33}│", ".env.taker");
    println!("\x1b[1m└──────────────────────────────────────────────────┘\x1b[0m");
    println!();
    println!("  \x1b[2mLogs: \x1b[33m[anvil]\x1b[0m");
    println!("  Press Ctrl-C to stop all services.");
    println!();

    // 10. Block until Ctrl-C.
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| SwapError::InvalidConfig(format!("signal error: {e}")))?;

    println!();
    eprintln!("  [\x1b[1minfra\x1b[0m] Shutting down...");

    // Anvil drops here.
    drop(anvil);

    Ok(())
}

struct EnvParams<'a> {
    eth_rpc_url: &'a str,
    eth_private_key: &'a str,
    eth_htlc_address: &'a str,
    lez_sequencer_url: &'a str,
    lez_auth: &'a LezAuth,
    lez_htlc_program_id: &'a str,
    lez_amount: u128,
    eth_amount: &'a str,
    eth_recipient: &'a str,
    lez_taker_account: &'a str,
    nssa_wallet_home_dir: &'a str,
}

fn write_env_file(path: &str, p: &EnvParams) -> Result<()> {
    let lez_auth_lines = match p.lez_auth {
        LezAuth::RawKey(key) => format!("LEZ_SIGNING_KEY={key}"),
        LezAuth::Wallet { home, account_id } => {
            let account_b58 = base58::ToBase58::to_base58(account_id.value().as_slice());
            format!(
                "LEZ_WALLET_HOME={}\nLEZ_ACCOUNT_ID={}",
                home.display(),
                account_b58,
            )
        }
    };

    let contents = format!(
        "\
# Auto-generated by `swap-cli infra` — do not edit while infra is running.

# Ethereum
ETH_RPC_URL={eth_rpc}
ETH_PRIVATE_KEY={eth_key}
ETH_HTLC_ADDRESS={eth_htlc}

# LEZ
LEZ_SEQUENCER_URL={lez_seq}
{lez_auth}
LEZ_HTLC_PROGRAM_ID={lez_prog}

# Swap parameters
LEZ_AMOUNT={lez_amount}
ETH_AMOUNT={eth_amount}

# Timelocks (absolute Unix timestamps)
ETH_TIMELOCK_MINUTES=10
LEZ_TIMELOCK_MINUTES=5

# Counterparty
ETH_RECIPIENT_ADDRESS={eth_recipient}
LEZ_TAKER_ACCOUNT_ID={lez_taker}

# Polling
POLL_INTERVAL_MS=500

# Wallet home (used by wallet::WalletCore::from_env)
NSSA_WALLET_HOME_DIR={wallet_home}
",
        eth_rpc = p.eth_rpc_url,
        eth_key = p.eth_private_key,
        eth_htlc = p.eth_htlc_address,
        lez_seq = p.lez_sequencer_url,
        lez_auth = lez_auth_lines,
        lez_prog = p.lez_htlc_program_id,
        lez_amount = p.lez_amount,
        eth_amount = p.eth_amount,
        eth_recipient = p.eth_recipient,
        lez_taker = p.lez_taker_account,
        wallet_home = p.nssa_wallet_home_dir,
    );

    let mut f = std::fs::File::create(path)
        .map_err(|e| SwapError::InvalidConfig(format!("failed to write {path}: {e}")))?;
    f.write_all(contents.as_bytes())
        .map_err(|e| SwapError::InvalidConfig(format!("failed to write {path}: {e}")))?;

    Ok(())
}
