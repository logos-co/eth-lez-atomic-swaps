//! The only part of the faucet that touches a key or a chain.
//!
//! Kept to two operations — read a balance, send a plain value transfer — so
//! the surface a compromised process could abuse is exactly the surface the
//! rate limits guard. No contract calls, no approvals, no `eth_sign`.

use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, U256, utils::format_units};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;

/// A funded hot key plus the provider that spends it.
#[derive(Clone)]
pub struct Chain {
    provider: DynProvider,
    address: Address,
    chain_id: u64,
}

impl Chain {
    /// Connect, learn the chain id, and report the faucet's own address.
    ///
    /// The chain id is read once here rather than per-send: it is what makes
    /// `/stats` able to name the network it is actually on, which is the
    /// cheapest possible guard against the "we funded the key on the wrong
    /// chain" mistake.
    pub async fn connect(rpc_url: &str, private_key: &str) -> Result<Self, String> {
        let signer: PrivateKeySigner = private_key
            .trim()
            .trim_start_matches("0x")
            .parse()
            .map_err(|_| {
                // Never echo the value — this message ends up in logs.
                "FAUCET_PRIVATE_KEY is not a valid secp256k1 key (want 64 hex chars)".to_string()
            })?;
        let address = signer.address();

        let url = rpc_url
            .parse()
            .map_err(|e| format!("FAUCET_RPC_URL is not a URL: {e}"))?;
        let provider = ProviderBuilder::new().wallet(signer).connect_http(url).erased();

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| format!("RPC handshake failed against {rpc_url}: {e}"))?;

        Ok(Self {
            provider,
            address,
            chain_id,
        })
    }

    pub fn address(&self) -> Address {
        self.address
    }

    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Balance in wei as a `u128`. Saturates rather than failing on the
    /// (impossible for a testnet faucet) overflow, so a balance read can
    /// never be the thing that takes `/health` down.
    pub async fn balance_wei(&self, of: Address) -> Result<u128, String> {
        let raw = self
            .provider
            .get_balance(of)
            .await
            .map_err(|e| format!("balance read failed: {e}"))?;
        Ok(u256_to_u128_saturating(raw))
    }

    /// Send `amount_wei` to `to` and wait for the receipt.
    ///
    /// Waiting (rather than returning the hash on submit) is deliberate: the
    /// ledger records a drip only after this returns, so a dropped or reverted
    /// send does not cost the user their daily claim. It also means the app
    /// can tell the user "it's on its way" with a hash that a block already
    /// contains, instead of one that may never land.
    pub async fn send_drip(&self, to: Address, amount_wei: u128) -> Result<String, String> {
        let tx = TransactionRequest::default()
            .with_to(to)
            .with_value(U256::from(amount_wei));

        let pending = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| format!("drip send failed: {e}"))?;
        let hash = *pending.tx_hash();

        let receipt = pending
            .get_receipt()
            .await
            .map_err(|e| format!("drip {hash} was submitted but its receipt never arrived: {e}"))?;

        if !receipt.status() {
            return Err(format!("drip {hash} reverted on-chain"));
        }
        Ok(format!("{hash:#x}"))
    }
}

fn u256_to_u128_saturating(value: U256) -> u128 {
    u128::try_from(value).unwrap_or(u128::MAX)
}

/// ETH with 6 decimals, for log lines only. `/stats` uses the exact
/// `format_wei_as_eth` from `config.rs` instead.
pub fn eth_for_logs(wei: u128) -> String {
    format_units(U256::from(wei), "ether")
        .map(|s| {
            s.trim_end_matches('0')
                .trim_end_matches('.')
                .to_string()
        })
        .unwrap_or_else(|_| format!("{wei} wei"))
}
