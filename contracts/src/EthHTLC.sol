// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

contract EthHTLC {
    enum SwapState {
        EMPTY,
        OPEN,
        CLAIMED,
        REFUNDED
    }

    struct HTLC {
        address payable sender;
        address payable recipient;
        uint256 amount;
        bytes32 hashlock;
        uint256 timelock;
        /// @dev LEZ AccountId (exactly 32 bytes) of the taker, i.e. the only
        ///      LEZ account permitted to claim the counterpart LEZ lock. It is
        ///      carried here so the maker learns it on-chain at lock time.
        bytes32 takerLezAccount;
        SwapState state;
    }

    /// @notice ABI generation of this contract. Clients MUST read this once at
    ///         startup and refuse to operate on a mismatch.
    ///
    /// @dev Why this exists. The `Locked` topic0 changed in generation 2, and a
    ///      topic mismatch is invisible in ordinary execution: filtering happens
    ///      at the RPC node, and a query that matches nothing legitimately
    ///      returns an empty array rather than an error. A maker pointed at a
    ///      generation-1 deployment would therefore not fail — it would poll
    ///      forever and simply never see a lock. Nothing the contract does
    ///      during `lock`/`claim`/`refund` can make that loud, so the signal has
    ///      to be a value the client reads explicitly and up front.
    ///
    ///      Generation 1 is the pre-`takerLezAccount` deployment (the shared
    ///      Sepolia EthHTLC). It has no `INTERFACE_VERSION`, so the startup call
    ///      reverts there instead of returning 1 — which is equally loud, and is
    ///      why generation 1 is never returned by any deployed contract.
    ///
    ///      Bump this on any change to the `lock`/`claim`/`refund` signatures,
    ///      the `HTLC` struct layout, or the `Locked`/`Claimed`/`Refunded` event
    ///      signatures.
    uint256 public constant INTERFACE_VERSION = 2;

    uint256 public immutable minTimelockDelta;

    mapping(bytes32 => HTLC) public htlcs;

    event Locked(
        bytes32 indexed swapId,
        address indexed sender,
        address indexed recipient,
        uint256 amount,
        bytes32 hashlock,
        uint256 timelock,
        bytes32 takerLezAccount
    );

    event Claimed(bytes32 indexed swapId, bytes32 preimage);

    event Refunded(bytes32 indexed swapId);

    error InvalidAmount();
    error InvalidHashLock();
    error InvalidTimelock();
    error InvalidRecipient();
    error InvalidTakerLezAccount();
    error SwapAlreadyExists();
    error SwapNotOpen();
    error InvalidPreimage();
    error TimelockNotExpired();
    error NotRecipient();
    error NotSender();
    error TransferFailed();

    constructor(uint256 _minTimelockDelta) {
        minTimelockDelta = _minTimelockDelta;
    }

    /// @notice Lock ETH into an HTLC.
    /// @param hashlock SHA-256 hash of the secret preimage.
    /// @param timelock Absolute Unix timestamp after which the sender can refund.
    /// @param recipient Address that can claim by revealing the preimage.
    /// @param takerLezAccount LEZ AccountId (32 bytes) of the taker, published
    ///        on-chain so the maker can name it as the designated claimant of
    ///        the counterpart LEZ lock. Must be non-zero.
    /// @return swapId Deterministic identifier for this HTLC.
    function lock(bytes32 hashlock, uint256 timelock, address payable recipient, bytes32 takerLezAccount)
        external
        payable
        returns (bytes32 swapId)
    {
        if (msg.value == 0) revert InvalidAmount();
        if (timelock <= block.timestamp + minTimelockDelta) revert InvalidTimelock();
        if (recipient == address(0)) revert InvalidRecipient();
        if (hashlock == bytes32(0)) revert InvalidHashLock();
        if (takerLezAccount == bytes32(0)) revert InvalidTakerLezAccount();

        // All components are fixed-width, so encodePacked is unambiguous.
        swapId = keccak256(abi.encodePacked(msg.sender, recipient, msg.value, hashlock, timelock, takerLezAccount));

        if (htlcs[swapId].state != SwapState.EMPTY) revert SwapAlreadyExists();

        htlcs[swapId] = HTLC({
            sender: payable(msg.sender),
            recipient: recipient,
            amount: msg.value,
            hashlock: hashlock,
            timelock: timelock,
            takerLezAccount: takerLezAccount,
            state: SwapState.OPEN
        });

        emit Locked(swapId, msg.sender, recipient, msg.value, hashlock, timelock, takerLezAccount);
    }

    /// @notice Claim locked ETH by revealing the preimage.
    /// @param swapId The HTLC identifier.
    /// @param preimage The secret whose SHA-256 hash matches the hashlock.
    function claim(bytes32 swapId, bytes32 preimage) external {
        HTLC storage htlc = htlcs[swapId];

        if (htlc.state != SwapState.OPEN) revert SwapNotOpen();
        if (msg.sender != htlc.recipient) revert NotRecipient();
        if (sha256(abi.encodePacked(preimage)) != htlc.hashlock) revert InvalidPreimage();

        htlc.state = SwapState.CLAIMED;

        emit Claimed(swapId, preimage);

        (bool success,) = htlc.recipient.call{value: htlc.amount}("");
        if (!success) revert TransferFailed();
    }

    /// @notice Refund locked ETH after the timelock has expired.
    /// @param swapId The HTLC identifier.
    function refund(bytes32 swapId) external {
        HTLC storage htlc = htlcs[swapId];

        if (htlc.state != SwapState.OPEN) revert SwapNotOpen();
        if (block.timestamp < htlc.timelock) revert TimelockNotExpired();
        if (msg.sender != htlc.sender) revert NotSender();

        htlc.state = SwapState.REFUNDED;

        emit Refunded(swapId);

        (bool success,) = htlc.sender.call{value: htlc.amount}("");
        if (!success) revert TransferFailed();
    }

    /// @notice Read the full HTLC state for a given swapId.
    /// @param swapId The HTLC identifier.
    /// @return The HTLC struct.
    function getHTLC(bytes32 swapId) external view returns (HTLC memory) {
        return htlcs[swapId];
    }
}
