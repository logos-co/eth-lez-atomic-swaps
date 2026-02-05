// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

contract EthHTLC {
    struct HTLC {
        address payable sender;
        address payable recipient;
        uint256 amount;
        bytes32 hashlock;
        uint256 timelock;
        bool claimed;
        bool refunded;
    }

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
    error InvalidTimelock();
    error InvalidRecipient();
    error SwapAlreadyExists();
    error SwapNotFound();
    error AlreadyClaimed();
    error AlreadyRefunded();
    error InvalidPreimage();
    error TimelockNotExpired();
    error NotRecipient();
    error NotSender();
    error TransferFailed();

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
        if (timelock <= block.timestamp) revert InvalidTimelock();
        if (recipient == address(0)) revert InvalidRecipient();

        swapId = keccak256(
            abi.encodePacked(msg.sender, recipient, msg.value, hashlock, timelock)
        );

        if (htlcs[swapId].amount != 0) revert SwapAlreadyExists();

        htlcs[swapId] = HTLC({
            sender: payable(msg.sender),
            recipient: recipient,
            amount: msg.value,
            hashlock: hashlock,
            timelock: timelock,
            claimed: false,
            refunded: false
        });

        emit Locked(swapId, msg.sender, recipient, msg.value, hashlock, timelock);
    }

    /// @notice Claim locked ETH by revealing the preimage.
    /// @param swapId The HTLC identifier.
    /// @param preimage The secret whose SHA-256 hash matches the hashlock.
    function claim(bytes32 swapId, bytes32 preimage) external {
        HTLC storage htlc = htlcs[swapId];

        if (htlc.amount == 0) revert SwapNotFound();
        if (htlc.claimed) revert AlreadyClaimed();
        if (htlc.refunded) revert AlreadyRefunded();
        if (msg.sender != htlc.recipient) revert NotRecipient();
        if (sha256(abi.encodePacked(preimage)) != htlc.hashlock) revert InvalidPreimage();

        htlc.claimed = true;

        emit Claimed(swapId, preimage);

        (bool success,) = htlc.recipient.call{value: htlc.amount}("");
        if (!success) revert TransferFailed();
    }

    /// @notice Refund locked ETH after the timelock has expired.
    /// @param swapId The HTLC identifier.
    function refund(bytes32 swapId) external {
        HTLC storage htlc = htlcs[swapId];

        if (htlc.amount == 0) revert SwapNotFound();
        if (htlc.claimed) revert AlreadyClaimed();
        if (htlc.refunded) revert AlreadyRefunded();
        if (block.timestamp < htlc.timelock) revert TimelockNotExpired();
        if (msg.sender != htlc.sender) revert NotSender();

        htlc.refunded = true;

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
