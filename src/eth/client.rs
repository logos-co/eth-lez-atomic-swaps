use alloy::{
    primitives::{Address, FixedBytes, U256},
    providers::{Provider, ProviderBuilder},
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

pub struct EthClient {
    contract: EthHTLC::EthHTLCInstance<alloy::providers::DynProvider>,
}

impl EthClient {
    pub fn new(config: &SwapConfig) -> Result<Self> {
        let signer: PrivateKeySigner = config
            .eth_private_key
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid ETH private key: {e}")))?;

        let rpc_url = config
            .eth_rpc_url
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid ETH_RPC_URL: {e}")))?;

        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect_http(rpc_url)
            .erased();

        let contract = EthHTLC::new(config.eth_htlc_address, provider);

        Ok(Self { contract })
    }

    /// Lock ETH into an HTLC. Returns the swap ID.
    pub async fn lock(
        &self,
        hashlock: [u8; 32],
        timelock: u64,
        recipient: Address,
        eth_amount: U256,
    ) -> Result<FixedBytes<32>> {
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

        Ok(log.inner.data.swapId)
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