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
        SwapState state;
    }

    uint256 public immutable minTimelockDelta;

    mapping(bytes32 => HTLC) public htlcs;

    event Locked(
        bytes32 indexed swapId,
        address indexed sender,
        address indexed recipient,
        uint256 amount,
        bytes32 hashlock,
        uint256 timelock
    );

    event Claimed(bytes32 indexed swapId, bytes32 preimage);

    event Refunded(bytes32 indexed swapId);

    error InvalidAmount();
    error InvalidHashLock();
    error InvalidTimelock();
    error InvalidRecipient();
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
    /// @return swapId Deterministic identifier for this HTLC.
    function lock(
        bytes32 hashlock,
        uint256 timelock,
        address payable recipient
    ) external payable returns (bytes32 swapId) {
        if (msg.value == 0) revert InvalidAmount();
        if (timelock <= block.timestamp + minTimelockDelta) revert InvalidTimelock();
        if (recipient == address(0)) revert InvalidRecipient();
        if (hashlock == bytes32(0)) revert InvalidHashLock();

        swapId = keccak256(
            abi.encodePacked(msg.sender, recipient, msg.value, hashlock, timelock)
        );

        if (htlcs[swapId].state != SwapState.EMPTY) revert SwapAlreadyExists();

        htlcs[swapId] = HTLC({
            sender: payable(msg.sender),
            recipient: recipient,
            amount: msg.value,
            hashlock: hashlock,
            timelock: timelock,
            state: SwapState.OPEN
        });

        emit Locked(swapId, msg.sender, recipient, msg.value, hashlock, timelock);
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
