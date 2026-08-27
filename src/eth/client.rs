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

/// Whether a failed `INTERFACE_VERSION` call is the DEPLOYMENT answering "no
/// such getter", as opposed to the endpoint failing to answer at all.
///
/// The distinction is the whole point: only the first says anything about which
/// version the address speaks. A 429, a 502 or a dropped connection establishes
/// nothing about the contract, and reporting it as an unversioned deployment
/// tells the operator to redeploy the venue over a transient RPC hiccup.
///
/// Erring towards "not a verdict" is the safe direction — a revert misreported
/// as an RPC fault still refuses to start and still quotes what the node said,
/// while an RPC fault misreported as a verdict is a false diagnosis.
fn reads_as_missing_getter(err: &alloy::contract::Error) -> bool {
    use alloy::contract::Error;
    match err {
        // The node answered `0x`, or answered with data this ABI cannot
        // decode: an address with no code, or not this EthHTLC.
        Error::ZeroData(..) | Error::AbiError(..) => true,
        // The node answered with a JSON-RPC error. Only an execution revert is
        // the contract speaking; every other code is the endpoint.
        Error::TransportError(e) => e.as_error_resp().is_some_and(|resp| {
            resp.as_revert_data().is_some()
                || resp
                    .message
                    .to_ascii_lowercase()
                    .contains("execution reverted")
        }),
        _ => false,
    }
}

/// Read `INTERFACE_VERSION` once and refuse to proceed on a skew. Called from
/// [`EthClient::new`] — i.e. before any watcher starts waiting and before any
/// funds move. One free `eth_call`; no key, no gas, no state dependency.
///
/// Returns the version the contract actually reported, so a caller that
/// records it as provenance (`swap-cli chain-report`) publishes the reading
/// rather than the constant it was checked against — the two can only drift if
/// the handshake is ever relaxed to accept a range, which is exactly when the
/// difference would start to matter.
pub async fn verify_interface_version<P: Provider>(provider: &P, address: Address) -> Result<u64> {
    let contract = EthHTLC::new(address, provider);
    let reported = match contract.INTERFACE_VERSION().call().await {
        Ok(v) => Some(v.saturating_to::<u64>()),
        // A revert / empty returndata means "no such getter" — an unversioned
        // (pre-v2) deployment, or not an EthHTLC at all. Both are fatal here,
        // and both get the same actionable remedy.
        Err(e) if reads_as_missing_getter(&e) => None,
        // The endpoint never answered, so this call established nothing about
        // the deployment. Saying it predates v2 would be a diagnosis the
        // handshake did not make.
        Err(e) => {
            return Err(SwapError::EthRpc(format!(
                "could not read INTERFACE_VERSION from the contract at {address}: {e}. That is a \
                 failure to reach the endpoint, not a verdict about the deployment — until the \
                 call is answered, this build cannot tell which interface version {address} \
                 speaks. Check ETH_RPC_URL is reachable and try again."
            )));
        }
    };

    match version_verdict(reported) {
        VersionVerdict::Match => Ok(reported.expect("a matched verdict carries a version")),
        other => Err(SwapError::EthAbiMismatch(version_mismatch_message(
            other, address,
        ))),
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

        // Single startup handshake, in order, one round trip each. The version
        // check goes FIRST and is one free eth_call, BEFORE any wait loop can
        // start and before any funds move: a contract/app version skew must fail
        // loudly here rather than manifest as a watcher that never sees an event
        // (see `verify_interface_version`). Only once the deployment is known to
        // be one we speak do we bother learning which chain it is on.
        verify_interface_version(&provider, config.eth_htlc_address).await?;

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
    ) -> Result<EthLockReceipt> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::sol_types::{SolCall as _, SolEvent as _};

    // Only the deployment's own answer may become a verdict about the
    // deployment. Telling an operator to redeploy the pinned public-trial venue
    // because one eth_call was rate-limited is a diagnosis the handshake never
    // made — and the pinned endpoint answers bursts with 429 by design.
    #[test]
    fn only_the_contracts_own_answer_reads_as_a_missing_getter() {
        use alloy::transports::{RpcError, TransportErrorKind};

        // Built from the wire JSON a node actually sends, so the shapes under
        // test are the ones the endpoint produces rather than a hand-made enum.
        let resp = |json: &str| {
            alloy::contract::Error::TransportError(RpcError::ErrorResp(
                serde_json::from_str(json).expect("a JSON-RPC error payload"),
            ))
        };

        // The contract spoke: a revert is "this ABI has no such function".
        assert!(reads_as_missing_getter(&resp(
            r#"{"code":3,"message":"execution reverted"}"#
        )));
        // Undecodable returndata is likewise an answer — just not this ABI's.
        assert!(reads_as_missing_getter(&alloy::contract::Error::AbiError(
            alloy::sol_types::Error::Overrun.into()
        )));

        // The endpoint spoke, and it said nothing about the contract. These are
        // the exact shapes the pinned endpoint produces under load.
        assert!(!reads_as_missing_getter(&resp(
            r#"{"code":-32005,"message":"Rate limit exceeded. To obtain higher limits, please request a personal token"}"#
        )));
        assert!(!reads_as_missing_getter(&resp(
            r#"{"code":-32603,"message":"Internal error"}"#
        )));
        assert!(!reads_as_missing_getter(&resp(
            r#"{"code":-32000,"message":"header not found"}"#
        )));

        // Nothing answered at all.
        assert!(!reads_as_missing_getter(
            &alloy::contract::Error::TransportError(RpcError::Transport(
                TransportErrorKind::BackendGone
            ))
        ));
        assert!(!reads_as_missing_getter(
            &alloy::contract::Error::TransportError(TransportErrorKind::custom_str(
                "connection refused"
            ))
        ));
    }

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
