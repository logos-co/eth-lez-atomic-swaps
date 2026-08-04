use alloy::{
    primitives::{Address, FixedBytes, U256},
    providers::{Provider, ProviderBuilder, WsConnect},
    signers::local::PrivateKeySigner,
    sol,
};

use crate::{
    config::SwapConfig,
    error::{Result, SwapError},
};

sol! {
    #[sol(rpc)]
    contract EthHTLC {
        enum SwapState { EMPTY, OPEN, CLAIMED, REFUNDED }

        struct HTLC {
            address sender;
            address recipient;
            uint256 amount;
            bytes32 hashlock;
            uint256 timelock;
            SwapState state;
        }

        event Locked(
            bytes32 indexed swapId,
            address indexed sender,
            address indexed recipient,
            uint256 amount,
            bytes32 hashlock,
            uint256 timelock,
        );
        event Claimed(bytes32 indexed swapId, bytes32 preimage);
        event Refunded(bytes32 indexed swapId);

        function lock(bytes32 hashlock, uint256 timelock, address recipient) external payable returns (bytes32 swapId);
        function claim(bytes32 swapId, bytes32 preimage) external;
        function refund(bytes32 swapId) external;
        function getHTLC(bytes32 swapId) external view returns (HTLC memory);
    }
}

/// Result of a successful [`EthClient::lock`] call: the swap ID extracted from
/// the `Locked` event log, plus the transaction hash of the lock tx itself.
/// The tx hash is what a consumer needs to link a receipt to a block explorer
/// — `receipt.transaction_hash` was previously computed and thrown away.
#[derive(Debug, Clone, Copy)]
pub struct EthLockReceipt {
    pub swap_id: FixedBytes<32>,
    pub tx_hash: FixedBytes<32>,
}

pub struct EthClient {
    contract: EthHTLC::EthHTLCInstance<alloy::providers::DynProvider>,
    chain_id: u64,
}

impl EthClient {
    pub async fn new(config: &SwapConfig) -> Result<Self> {
        let signer: PrivateKeySigner = config
            .eth_private_key
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid ETH private key: {e}")))?;

        let ws = WsConnect::new(&config.eth_rpc_url);

        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect_ws(ws)
            .await
            .map_err(|e| SwapError::EthRpc(format!("WebSocket connect failed: {e}")))?
            .erased();

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?;

        let contract = EthHTLC::new(config.eth_htlc_address, provider);

        Ok(Self { contract, chain_id })
    }

    /// The chain ID of the connected ETH endpoint, so a consumer can tell a
    /// Sepolia tx hash from an Anvil one when building an explorer link.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Lock ETH into an HTLC. Returns the swap ID and the lock tx hash.
    pub async fn lock(
        &self,
        hashlock: [u8; 32],
        timelock: u64,
        recipient: Address,
        eth_amount: U256,
    ) -> Result<EthLockReceipt> {
        let receipt = self
            .contract
            .lock(hashlock.into(), U256::from(timelock), recipient)
            .value(eth_amount)
            .send()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?
            .get_receipt()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?;

        // Extract swapId from the Locked event log.
        let log = receipt
            .inner
            .logs()
            .iter()
            .find_map(|log| log.log_decode::<EthHTLC::Locked>().ok())
            .ok_or_else(|| SwapError::EthReverted("no Locked event in receipt".into()))?;

        Ok(EthLockReceipt {
            swap_id: log.inner.data.swapId,
            tx_hash: receipt.transaction_hash,
        })
    }

    /// Claim locked ETH by revealing the preimage. Returns the tx hash.
    pub async fn claim(
        &self,
        swap_id: FixedBytes<32>,
        preimage: [u8; 32],
    ) -> Result<FixedBytes<32>> {
        let receipt = self
            .contract
            .claim(swap_id, preimage.into())
            .send()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?
            .get_receipt()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?;

        Ok(receipt.transaction_hash)
    }

    /// Refund locked ETH after timelock expiry. Returns the tx hash.
    pub async fn refund(&self, swap_id: FixedBytes<32>) -> Result<FixedBytes<32>> {
        let receipt = self
            .contract
            .refund(swap_id)
            .send()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?
            .get_receipt()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?;

        Ok(receipt.transaction_hash)
    }

    /// Read the on-chain HTLC state for a given swap ID.
    pub async fn get_htlc(&self, swap_id: FixedBytes<32>) -> Result<EthHTLC::HTLC> {
        let htlc = self
            .contract
            .getHTLC(swap_id)
            .call()
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?;

        Ok(htlc)
    }

    pub fn contract_address(&self) -> Address {
        *self.contract.address()
    }

    pub fn provider(&self) -> &alloy::providers::DynProvider {
        self.contract.provider()
    }
}
