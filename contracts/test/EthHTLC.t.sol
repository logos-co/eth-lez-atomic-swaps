// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {EthHTLC} from "../src/EthHTLC.sol";

contract EthHTLCTest is Test {
    EthHTLC public htlc;

    address payable taker;
    address payable maker;

    bytes32 constant PREIMAGE = "secret_preimage_for_testing_1234";
    bytes32 HASHLOCK;
    uint256 TIMELOCK;
    uint256 constant AMOUNT = 1 ether;

    function setUp() public {
        htlc = new EthHTLC(300);
        taker = payable(makeAddr("taker"));
        maker = payable(makeAddr("maker"));
        vm.deal(taker, 10 ether);
        vm.deal(maker, 10 ether);
        HASHLOCK = sha256(abi.encodePacked(PREIMAGE));
        TIMELOCK = block.timestamp + 600;
    }

    function _lockDefault() internal returns (bytes32 swapId) {
        vm.prank(taker);
        swapId = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker);
    }

    // -------------------------------------------------------------------------
    // Happy-path: lock
    // -------------------------------------------------------------------------

    function test_lock_succeeds() public {
        uint256 contractBalBefore = address(htlc).balance;

        vm.expectEmit(true, true, true, true);
        bytes32 expectedId = keccak256(
            abi.encodePacked(taker, maker, AMOUNT, HASHLOCK, TIMELOCK)
        );
        emit EthHTLC.Locked(expectedId, taker, maker, AMOUNT, HASHLOCK, TIMELOCK);

        bytes32 swapId = _lockDefault();

        assertEq(address(htlc).balance, contractBalBefore + AMOUNT);
        assertEq(swapId, expectedId);
    }

    function test_lock_computesCorrectSwapId() public {
        bytes32 swapId = _lockDefault();
        bytes32 expected = keccak256(
            abi.encodePacked(taker, maker, AMOUNT, HASHLOCK, TIMELOCK)
        );
        assertEq(swapId, expected);
    }

    // -------------------------------------------------------------------------
    // Happy-path: claim
    // -------------------------------------------------------------------------

    function test_claim_succeeds() public {
        bytes32 swapId = _lockDefault();

        uint256 makerBalBefore = maker.balance;

        vm.prank(maker);
        htlc.claim(swapId, PREIMAGE);

        assertEq(maker.balance, makerBalBefore + AMOUNT);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.CLAIMED));
    }

    function test_claim_emitsPreimageInEvent() public {
        bytes32 swapId = _lockDefault();

        vm.expectEmit(true, false, false, true);
        emit EthHTLC.Claimed(swapId, PREIMAGE);

        vm.prank(maker);
        htlc.claim(swapId, PREIMAGE);
    }

    // -------------------------------------------------------------------------
    // getHTLC
    // -------------------------------------------------------------------------

    function test_getHTLC_returnsCorrectState() public {
        bytes32 swapId = _lockDefault();

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);

        assertEq(h.sender, taker);
        assertEq(h.recipient, maker);
        assertEq(h.amount, AMOUNT);
        assertEq(h.hashlock, HASHLOCK);
        assertEq(h.timelock, TIMELOCK);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.OPEN));
    }

    // -------------------------------------------------------------------------
    // Happy-path: refund
    // -------------------------------------------------------------------------

    function test_refund_succeedsAfterTimelock() public {
        bytes32 swapId = _lockDefault();

        uint256 takerBalBefore = taker.balance;

        vm.warp(TIMELOCK + 1);

        vm.expectEmit(true, false, false, false);
        emit EthHTLC.Refunded(swapId);

        vm.prank(taker);
        htlc.refund(swapId);

        assertEq(taker.balance, takerBalBefore + AMOUNT);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.REFUNDED));
    }

    function test_refund_succeedsAtExactTimelock() public {
        bytes32 swapId = _lockDefault();

        vm.warp(TIMELOCK);

        vm.prank(taker);
        htlc.refund(swapId);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.REFUNDED));
    }

    function test_refund_revertsBeforeTimelock() public {
        bytes32 swapId = _lockDefault();

        vm.prank(taker);
        vm.expectRevert(EthHTLC.TimelockNotExpired.selector);
        htlc.refund(swapId);
    }

    // -------------------------------------------------------------------------
    // Failure cases: lock
    // -------------------------------------------------------------------------

    function test_lock_revertsWithZeroValue() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidAmount.selector);
        htlc.lock{value: 0}(HASHLOCK, TIMELOCK, maker);
    }

    function test_lock_revertsWithPastTimelock() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidTimelock.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, block.timestamp, maker);
    }

    function test_lock_revertsWithInsufficientTimelockDelta() public {
        uint256 delta = htlc.minTimelockDelta();
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidTimelock.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, block.timestamp + delta, maker);
    }

    function test_lock_revertsWithZeroHashlock() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidHashLock.selector);
        htlc.lock{value: AMOUNT}(bytes32(0), TIMELOCK, maker);
    }

    function test_lock_revertsWithZeroRecipient() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidRecipient.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, payable(address(0)));
    }

    function test_lock_revertsOnDuplicate() public {
        _lockDefault();

        vm.deal(taker, 10 ether);
        vm.prank(taker);
        vm.expectRevert(EthHTLC.SwapAlreadyExists.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker);
    }

    // -------------------------------------------------------------------------
    // Failure cases: claim
    // -------------------------------------------------------------------------

    function test_claim_revertsWithWrongPreimage() public {
        bytes32 swapId = _lockDefault();

        vm.prank(maker);
        vm.expectRevert(EthHTLC.InvalidPreimage.selector);
        htlc.claim(swapId, bytes32("wrong_preimage_value_here_!!!!!!"));
    }

    function test_claim_revertsWhenNotRecipient() public {
        bytes32 swapId = _lockDefault();

        vm.prank(taker);
        vm.expectRevert(EthHTLC.NotRecipient.selector);
        htlc.claim(swapId, PREIMAGE);
    }

    function test_claim_revertsWhenAlreadyClaimed() public {
        bytes32 swapId = _lockDefault();

        vm.prank(maker);
        htlc.claim(swapId, PREIMAGE);

        vm.prank(maker);
        vm.expectRevert(EthHTLC.SwapNotOpen.selector);
        htlc.claim(swapId, PREIMAGE);
    }

    function test_claim_revertsWhenAlreadyRefunded() public {
        bytes32 swapId = _lockDefault();

        vm.warp(TIMELOCK);
        vm.prank(taker);
        htlc.refund(swapId);

        vm.prank(maker);
        vm.expectRevert(EthHTLC.SwapNotOpen.selector);
        htlc.claim(swapId, PREIMAGE);
    }

    function test_claim_revertsForNonexistentSwap() public {
        vm.prank(maker);
        vm.expectRevert(EthHTLC.SwapNotOpen.selector);
        htlc.claim(bytes32(uint256(0xdead)), PREIMAGE);
    }

    // -------------------------------------------------------------------------
    // Failure cases: refund
    // -------------------------------------------------------------------------

    function test_refund_revertsWhenNotSender() public {
        bytes32 swapId = _lockDefault();

        vm.warp(TIMELOCK);

        vm.prank(maker);
        vm.expectRevert(EthHTLC.NotSender.selector);
        htlc.refund(swapId);
    }

    function test_refund_revertsWhenAlreadyRefunded() public {
        bytes32 swapId = _lockDefault();

        vm.warp(TIMELOCK);
        vm.prank(taker);
        htlc.refund(swapId);

        vm.prank(taker);
        vm.expectRevert(EthHTLC.SwapNotOpen.selector);
        htlc.refund(swapId);
    }

    function test_refund_revertsWhenAlreadyClaimed() public {
        bytes32 swapId = _lockDefault();

        vm.prank(maker);
        htlc.claim(swapId, PREIMAGE);

        vm.warp(TIMELOCK);
        vm.prank(taker);
        vm.expectRevert(EthHTLC.SwapNotOpen.selector);
        htlc.refund(swapId);
    }

    function test_refund_revertsForNonexistentSwap() public {
        vm.warp(TIMELOCK);
        vm.prank(taker);
        vm.expectRevert(EthHTLC.SwapNotOpen.selector);
        htlc.refund(bytes32(uint256(0xdead)));
    }

    // -------------------------------------------------------------------------
    // Failure case: TransferFailed
    // -------------------------------------------------------------------------

    function test_claim_revertsWhenRecipientRejectsETH() public {
        RejectETH rejector = new RejectETH();
        address payable badRecipient = payable(address(rejector));

        vm.prank(taker);
        bytes32 swapId = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, badRecipient);

        vm.prank(badRecipient);
        vm.expectRevert(EthHTLC.TransferFailed.selector);
        htlc.claim(swapId, PREIMAGE);
    }

    function test_refund_revertsWhenSenderRejectsETH() public {
        RejectETH rejector = new RejectETH();
        address payable badSender = payable(address(rejector));
        vm.deal(badSender, 10 ether);

        vm.prank(badSender);
        htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker);

        bytes32 swapId = keccak256(
            abi.encodePacked(badSender, maker, AMOUNT, HASHLOCK, TIMELOCK)
        );

        vm.warp(TIMELOCK);
        vm.prank(badSender);
        vm.expectRevert(EthHTLC.TransferFailed.selector);
        htlc.refund(swapId);
    }

    // -------------------------------------------------------------------------
    // Cross-chain compatibility: shared test vectors with LEZ HTLC
    // -------------------------------------------------------------------------
    // These constants must match the Rust test suite in
    // programs/lez-htlc/methods/guest/src/main.rs (mod tests).
    // If either side changes SHA-256 behavior, one of these tests will break.

    bytes32 constant XCHAIN_PREIMAGE = "secret_preimage_for_testing_1234";
    bytes32 constant XCHAIN_HASHLOCK = 0x0ef69611a91e0805079387fee0b89fb7d6fcd505220d407bacaa40ce031745df;

    function test_crossChain_sha256Compatibility() public pure {
        // Verify that Solidity's sha256 of our shared preimage matches
        // the hardcoded hashlock (same value asserted in the Rust tests).
        bytes32 computed = sha256(abi.encodePacked(XCHAIN_PREIMAGE));
        assertEq(computed, XCHAIN_HASHLOCK);
    }

    function test_crossChain_lockAndClaimWithSharedPreimage() public {
        // Simulate the Ethereum side of a cross-chain atomic swap.
        // Taker locks ETH using the Maker's hashlock.
        // Maker claims ETH by revealing the preimage.
        // The same preimage is used on the LEZ side to claim lambda.
        uint256 timelock = block.timestamp + 600;

        vm.prank(taker);
        bytes32 swapId = htlc.lock{value: AMOUNT}(XCHAIN_HASHLOCK, timelock, maker);

        // Maker claims by revealing the preimage (as they would after locking on LEZ)
        vm.prank(maker);
        htlc.claim(swapId, XCHAIN_PREIMAGE);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.CLAIMED));
    }

    function test_crossChain_refundAfterTimeout() public {
        // Taker locks ETH, but Maker never claims (disappeared).
        // After timelock, Taker refunds. On LEZ side, Maker also refunds.
        uint256 timelock = block.timestamp + 600;

        vm.prank(taker);
        bytes32 swapId = htlc.lock{value: AMOUNT}(XCHAIN_HASHLOCK, timelock, maker);

        // Timelock expires, Taker reclaims ETH
        vm.warp(timelock);
        vm.prank(taker);
        htlc.refund(swapId);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.REFUNDED));
    }

    // -------------------------------------------------------------------------
    // Edge case: multiple concurrent swaps
    // -------------------------------------------------------------------------

    function test_multipleConcurrentSwaps() public {
        // Swap 1: taker -> maker, default params
        bytes32 swapId1 = _lockDefault();

        // Swap 2: different params (different timelock)
        uint256 timelock2 = TIMELOCK + 300;
        vm.prank(taker);
        bytes32 swapId2 = htlc.lock{value: 2 ether}(HASHLOCK, timelock2, maker);

        assertTrue(swapId1 != swapId2);

        // Claim swap 1
        vm.prank(maker);
        htlc.claim(swapId1, PREIMAGE);

        EthHTLC.HTLC memory h1 = htlc.getHTLC(swapId1);
        assertEq(uint8(h1.state), uint8(EthHTLC.SwapState.CLAIMED));

        // Swap 2 should be unaffected
        EthHTLC.HTLC memory h2 = htlc.getHTLC(swapId2);
        assertEq(uint8(h2.state), uint8(EthHTLC.SwapState.OPEN));

        // Refund swap 2
        vm.warp(timelock2);
        vm.prank(taker);
        htlc.refund(swapId2);

        h2 = htlc.getHTLC(swapId2);
        assertEq(uint8(h2.state), uint8(EthHTLC.SwapState.REFUNDED));
    }

    // -------------------------------------------------------------------------
    // Fuzz tests
    // -------------------------------------------------------------------------

    function testFuzz_lockClaimRoundtrip(uint256 amount, uint256 timelockDelta, bytes32 preimage) public {
        amount = bound(amount, 1, 10 ether);
        timelockDelta = bound(timelockDelta, 301, 365 days);
        vm.assume(preimage != bytes32(0));

        bytes32 fuzzHashlock = sha256(abi.encodePacked(preimage));
        uint256 fuzzTimelock = block.timestamp + timelockDelta;

        vm.deal(taker, amount);
        vm.prank(taker);
        bytes32 swapId = htlc.lock{value: amount}(fuzzHashlock, fuzzTimelock, maker);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(h.amount, amount);
        assertEq(h.hashlock, fuzzHashlock);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.OPEN));

        uint256 makerBalBefore = maker.balance;
        vm.prank(maker);
        htlc.claim(swapId, preimage);

        assertEq(maker.balance, makerBalBefore + amount);
        h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.CLAIMED));
    }

    function testFuzz_lockRefundRoundtrip(uint256 amount, uint256 timelockDelta) public {
        amount = bound(amount, 1, 10 ether);
        timelockDelta = bound(timelockDelta, 301, 365 days);

        uint256 fuzzTimelock = block.timestamp + timelockDelta;

        vm.deal(taker, amount);
        vm.prank(taker);
        bytes32 swapId = htlc.lock{value: amount}(HASHLOCK, fuzzTimelock, maker);

        uint256 takerBalBefore = taker.balance;
        vm.warp(fuzzTimelock);
        vm.prank(taker);
        htlc.refund(swapId);

        assertEq(taker.balance, takerBalBefore + amount);
        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.REFUNDED));
    }
}

/// @dev Helper contract that rejects all ETH transfers.
contract RejectETH {
    receive() external payable {
        revert();
    }
}
