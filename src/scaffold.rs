use std::path::{Path, PathBuf};

use lee::AccountId;

use crate::error::{Result, SwapError};

const WALLET_HOME_REL: &str = ".scaffold/wallet";

/// Parsed public account from the scaffold wallet config.
pub struct WalletAccount {
    pub account_id: AccountId,
    pub account_id_b58: String,
}

/// Path to the scaffold wallet home directory relative to the current
/// working directory. CLI commands are expected to run from the project root.
pub fn wallet_home() -> PathBuf {
    PathBuf::from(WALLET_HOME_REL)
}

/// Find the wallet config file. Scaffold creates `config.json`;
/// the wallet crate expects `wallet_config.json`. Check both.
fn wallet_config_path(wallet_home: &Path) -> Option<PathBuf> {
    let candidates = [
        wallet_home.join("config.json"),
        wallet_home.join("wallet_config.json"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Create a `WalletCore` from a scaffold wallet home directory.
/// Handles config.json vs wallet_config.json resolution and storage initialization.
pub fn wallet_core(home: &Path) -> Result<wallet::WalletCore> {
    let abs_home = std::fs::canonicalize(home).map_err(|_| {
        SwapError::Scaffold(format!(
            "wallet home '{}' not found — run `make setup` first",
            home.display()
        ))
    })?;

    let config_path = wallet_config_path(&abs_home).ok_or_else(|| {
        SwapError::Scaffold(format!(
            "no wallet config in '{}' — run `make setup` first",
            abs_home.display()
        ))
    })?;

    let storage_path = abs_home.join("storage.json");

    if storage_path.exists() {
        wallet::WalletCore::new_update_chain(config_path, storage_path, None)
    } else {
        // rc5: new_init_storage takes a password and also returns the mnemonic;
        // we use an empty password and discard the mnemonic (throwaway wallets).
        wallet::WalletCore::new_init_storage(config_path, storage_path, None, "")
            .map(|(wc, _mnemonic)| wc)
    }
    .map_err(|e| SwapError::Scaffold(format!("failed to initialize wallet: {e}")))
}

/// Extract public accounts from the wallet's key chain, creating new ones if
/// fewer than 2 exist (rc5 wallets no longer carry `initial_accounts` in
/// config — accounts live in the HD key chain inside storage).
/// Returns at least 2 accounts (maker + taker).
pub fn public_accounts(wc: &mut wallet::WalletCore) -> Result<Vec<WalletAccount>> {
    let mut ids: Vec<_> = wc
        .storage()
        .key_chain()
        .public_account_ids()
        .map(|(id, _chain_index)| id)
        .collect();

    while ids.len() < 2 {
        let (id, _chain_index) = wc.create_new_account_public(None);
        wc.store_persistent_data()
            .map_err(|e| SwapError::Scaffold(format!("failed to persist wallet storage: {e}")))?;
        ids.push(id);
    }

    Ok(ids
        .into_iter()
        .map(|account_id| {
            let account_id_b58 = base58::ToBase58::to_base58(account_id.value().as_slice());
            WalletAccount {
                account_id,
                account_id_b58,
            }
        })
        .collect())
}

/// Get the sequencer URL string from a WalletCore's config.
pub fn sequencer_url_of(wc: &wallet::WalletCore) -> String {
    wc.config().sequencer_addr.to_string()
}

/// Check whether the scaffold localnet is already running.
pub async fn localnet_is_running() -> bool {
    let output = tokio::process::Command::new("logos-scaffold")
        .args(["localnet", "status", "--json"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            serde_json::from_str::<serde_json::Value>(&stdout)
                .ok()
                .and_then(|v| v["ready"].as_bool())
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// Shell out to `logos-scaffold localnet start`. Skips if already running.
pub async fn localnet_start() -> Result<()> {
    if localnet_is_running().await {
        return Ok(());
    }

    let output = tokio::process::Command::new("logos-scaffold")
        .args(["localnet", "start"])
        .output()
        .await
        .map_err(|e| SwapError::Scaffold(format!("failed to run logos-scaffold: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SwapError::Scaffold(format!(
            "localnet start failed: {stderr}"
        )));
    }
    Ok(())
}

/// Shell out to `logos-scaffold localnet stop`.
/// Logs a warning on failure but does not return an error (best-effort cleanup).
pub async fn localnet_stop() {
    match tokio::process::Command::new("logos-scaffold")
        .args(["localnet", "stop"])
        .output()
        .await
    {
        Ok(output) if !output.status.success() => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("  \x1b[33mwarning\x1b[0m: localnet stop failed: {stderr}");
        }
        Err(e) => {
            eprintln!("  \x1b[33mwarning\x1b[0m: failed to run logos-scaffold: {e}");
        }
        _ => {}
    }
}

/// Shell out to `logos-scaffold wallet topup [address]`.
pub async fn wallet_topup(address: Option<&str>) -> Result<()> {
    let mut cmd = tokio::process::Command::new("logos-scaffold");
    cmd.args(["wallet", "topup"]);
    if let Some(addr) = address {
        cmd.arg(addr);
    }

    let output = cmd
        .output()
        .await
        .map_err(|e| SwapError::Scaffold(format!("wallet topup failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SwapError::Scaffold(format!(
            "wallet topup failed: {stderr}"
        )));
    }
    Ok(())
}
