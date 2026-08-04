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
            bytes32 takerLezAccount;
            SwapState state;
        }

        event Locked(
            bytes32 indexed swapId,
            address indexed sender,
            address indexed recipient,
            uint256 amount,
            bytes32 hashlock,
            uint256 timelock,
            bytes32 takerLezAccount,
        );
        event Claimed(bytes32 indexed swapId, bytes32 preimage);
        event Refunded(bytes32 indexed swapId);

        function INTERFACE_VERSION() external view returns (uint256);
        function lock(bytes32 hashlock, uint256 timelock, address recipient, bytes32 takerLezAccount) external payable returns (bytes32 swapId);
        function claim(bytes32 swapId, bytes32 preimage) external;
        function refund(bytes32 swapId) external;
        function getHTLC(bytes32 swapId) external view returns (HTLC memory);
    }
}

// ---------------------------------------------------------------------------
// Interface-version handshake (loud failure on a contract/app version skew)
// ---------------------------------------------------------------------------
//
// `lock()`'s selector and `Locked`'s topic0 both changed when `takerLezAccount`
// was added. A client on the WRONG side of that change does not error — it goes
// DEAF. Log filtering happens at the RPC node, and an empty match set is a
// perfectly valid answer, so `Locked_filter().query()` returns `Ok(vec![])`
// forever: nothing in ordinary contract execution can turn a topic mismatch
// into an error. The maker sits at "WaitingForEthLock" while takers lock ETH in
// front of it and burn their timelocks. For a public migration that is the
// worst possible failure mode — silent, and indistinguishable from an idle
// market.
//
// Since the chain cannot tell us, we ASK. The contract exposes
// `INTERFACE_VERSION` (a free constant getter) and every client reads it ONCE
// at construction, before any wait loop can start or any funds can move, and
// hard-fails on a mismatch with an actionable message.
//
// Deliberately NOT done: emitting a legacy-shaped `Locked` alongside the new
// one for "compatibility". An old maker would then match the legacy event and
// act WITHOUT reading the taker's authenticated LEZ account — locking LEZ to
// its statically configured counterparty while serving a stranger, which
// strands the escrow. Failing loudly is strictly better than working wrongly.

/// The `INTERFACE_VERSION` this build speaks. v2 = `takerLezAccount` bound into
/// `lock()`, `Locked` and the `swapId` preimage. Bump in lockstep with the
/// contract's constant on every breaking interface change.
pub const EXPECTED_INTERFACE_VERSION: u64 = 2;

/// Outcome of the startup handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionVerdict {
    /// The deployment speaks exactly this build's interface.
    Match,
    /// It answered with a different version — a real skew, either direction.
    Mismatch(u64),
    /// No `INTERFACE_VERSION` getter at all: a pre-versioned deployment, i.e.
    /// older than the taker-LEZ-account change (or not an EthHTLC).
    Unversioned,
}

/// Pure classification of the handshake result, so the skew decision is
/// unit-testable without a chain.
pub(crate) fn version_verdict(reported: Option<u64>) -> VersionVerdict {
    match reported {
        Some(v) if v == EXPECTED_INTERFACE_VERSION => VersionVerdict::Match,
        Some(v) => VersionVerdict::Mismatch(v),
        None => VersionVerdict::Unversioned,
    }
}

/// Actionable message for a verdict this build cannot work with.
fn version_mismatch_message(verdict: VersionVerdict, address: Address) -> String {
    match verdict {
        VersionVerdict::Match => unreachable!("a match is not a mismatch"),
        VersionVerdict::Mismatch(found) => format!(
            "the EthHTLC at {address} reports INTERFACE_VERSION {found}, but this app build speaks \
             {EXPECTED_INTERFACE_VERSION}. Refusing to start: the two disagree on lock()'s \
             arguments and on Locked's topic0, so this build would silently never see that \
             contract's lock events (an unmatched log filter returns an empty list, not an \
             error). Point ETH_HTLC_ADDRESS at a v{EXPECTED_INTERFACE_VERSION} deployment, or run \
             an app build that speaks v{found}."
        ),
        VersionVerdict::Unversioned => format!(
            "the contract at {address} has no INTERFACE_VERSION getter, so it predates the \
             taker-LEZ-account change (this app build expects v{EXPECTED_INTERFACE_VERSION}). \
             Against it this build would wait forever instead of seeing lock events. Redeploy \
             EthHTLC and update ETH_HTLC_ADDRESS, or run an older app build. (If you believe the \
             address IS current, check ETH_RPC_URL points at the right network.)"
        ),
    }
}

/// Read `INTERFACE_VERSION` once and refuse to proceed on a skew. Called from
/// [`EthClient::new`] — i.e. before any watcher starts waiting and before any
/// funds move. One free `eth_call`; no key, no gas, no state dependency.
pub async fn verify_interface_version<P: Provider>(provider: &P, address: Address) -> Result<()> {
    let contract = EthHTLC::new(address, provider);
    // A revert / empty returndata means "no such getter" — an unversioned
    // (pre-v2) deployment, or not an EthHTLC at all. Both are fatal here, and
    // both get the same actionable remedy.
    let reported = contract
        .INTERFACE_VERSION()
        .call()
        .await
        .ok()
        .map(|v| v.saturating_to::<u64>());

    match version_verdict(reported) {
        VersionVerdict::Match => Ok(()),
        other => Err(SwapError::EthAbiMismatch(version_mismatch_message(
            other, address,
        ))),
    }
}

pub struct EthClient {
    contract: EthHTLC::EthHTLCInstance<alloy::providers::DynProvider>,
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

        // One free eth_call, BEFORE any wait loop can start and before any funds
        // move: a contract/app version skew must fail loudly here rather than
        // manifest as a watcher that never sees an event (see
        // `verify_interface_version`).
        verify_interface_version(&provider, config.eth_htlc_address).await?;

        let contract = EthHTLC::new(config.eth_htlc_address, provider);

        Ok(Self { contract })
    }

    /// Lock ETH into an HTLC. Returns the swap ID.
    ///
    /// `taker_lez_account` is the LEZ AccountId that will be published on-chain
    /// as the only account allowed to claim the counterpart LEZ escrow — the
    /// taker's OWN account. The maker reads it off the `Locked` event, which is
    /// what lets a maker serve a stranger.
    pub async fn lock(
        &self,
        hashlock: [u8; 32],
        timelock: u64,
        recipient: Address,
        taker_lez_account: [u8; 32],
        eth_amount: U256,
    ) -> Result<FixedBytes<32>> {
        let receipt = self
            .contract
            .lock(
                hashlock.into(),
                U256::from(timelock),
                recipient,
                taker_lez_account.into(),
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall as _, SolEvent as _};

    // The handshake's whole job is to tell "the contract I speak" apart from
    // "the contract that would make my watcher silently deaf".
    #[test]
    fn version_verdict_matches_only_the_expected_version() {
        assert_eq!(
            version_verdict(Some(EXPECTED_INTERFACE_VERSION)),
            VersionVerdict::Match
        );
        // A newer contract is just as fatal as an older one — the skew is
        // symmetric, and in BOTH directions the symptom is an empty log filter.
        assert_eq!(
            version_verdict(Some(EXPECTED_INTERFACE_VERSION + 1)),
            VersionVerdict::Mismatch(EXPECTED_INTERFACE_VERSION + 1)
        );
        assert_eq!(version_verdict(Some(1)), VersionVerdict::Mismatch(1));
        // No getter at all: the pre-versioned deployment.
        assert_eq!(version_verdict(None), VersionVerdict::Unversioned);
    }

    // These messages fire during a public migration, so they must name the
    // actual remedy — not just "mismatch".
    #[test]
    fn version_mismatch_messages_are_actionable() {
        let unversioned = version_mismatch_message(VersionVerdict::Unversioned, Address::ZERO);
        assert!(unversioned.contains("INTERFACE_VERSION"));
        assert!(unversioned.contains("ETH_HTLC_ADDRESS"));
        assert!(unversioned.contains("wait forever"));

        let skew = version_mismatch_message(VersionVerdict::Mismatch(3), Address::ZERO);
        assert!(skew.contains("reports INTERFACE_VERSION 3"));
        assert!(skew.contains("Refusing to start"));
    }

    // Guards the premise of the probe: the ABI this build compiles against is
    // the 4-arg lock / 7-field HTLC one. If someone edits the sol! block back,
    // these constants (and the probe) must be revisited.
    #[test]
    fn compiled_abi_is_the_taker_lez_account_shape() {
        assert_eq!(
            EthHTLC::lockCall::SIGNATURE,
            "lock(bytes32,uint256,address,bytes32)"
        );
        assert_eq!(
            EthHTLC::Locked::SIGNATURE,
            "Locked(bytes32,address,address,uint256,bytes32,uint256,bytes32)"
        );
    }
}
