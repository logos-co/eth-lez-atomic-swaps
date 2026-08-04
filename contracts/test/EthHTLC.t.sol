// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {EthHTLC} from "../src/EthHTLC.sol";

contract EthHTLCTest is Test {
    EthHTLC public htlc;

    address payable taker;
    address payable maker;

    bytes32 constant PREIMAGE = "secret_preimage_for_testing_1234";
    /// @dev Stand-ins for LEZ AccountIds (exactly 32 bytes each).
    bytes32 constant TAKER_LEZ = bytes32(uint256(0xA11CE));
    bytes32 constant TAKER_LEZ_ALT = bytes32(uint256(0xB0B));
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
        swapId = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, TAKER_LEZ);
    }

    // -------------------------------------------------------------------------
    // Happy-path: lock
    // -------------------------------------------------------------------------

    function test_lock_succeeds() public {
        uint256 contractBalBefore = address(htlc).balance;

        vm.expectEmit(true, true, true, true);
        bytes32 expectedId = keccak256(abi.encodePacked(taker, maker, AMOUNT, HASHLOCK, TIMELOCK, TAKER_LEZ));
        emit EthHTLC.Locked(expectedId, taker, maker, AMOUNT, HASHLOCK, TIMELOCK, TAKER_LEZ);

        bytes32 swapId = _lockDefault();

        assertEq(address(htlc).balance, contractBalBefore + AMOUNT);
        assertEq(swapId, expectedId);
    }

    function test_lock_computesCorrectSwapId() public {
        bytes32 swapId = _lockDefault();
        bytes32 expected = keccak256(abi.encodePacked(taker, maker, AMOUNT, HASHLOCK, TIMELOCK, TAKER_LEZ));
        assertEq(swapId, expected);
    }

    // -------------------------------------------------------------------------
    // Version handshake
    // -------------------------------------------------------------------------

    /// @dev Pins the constant clients read at startup to detect a stale
    ///      deployment. If you change the ABI and this test still passes, you
    ///      forgot to bump INTERFACE_VERSION — which is the whole failure this
    ///      constant exists to prevent, since a stale topic filter returns an
    ///      empty result set rather than an error.
    function test_interfaceVersion_isPinned() public view {
        assertEq(htlc.INTERFACE_VERSION(), 2);
    }

    // -------------------------------------------------------------------------
    // takerLezAccount: swapId must bind the taker's LEZ account
    // -------------------------------------------------------------------------

    /// @dev The property the maker's matching logic depends on: two locks that
    ///      are identical in EVERY parameter except takerLezAccount must still
    ///      produce distinct swap ids, so both can coexist and each maps back
    ///      to exactly one designated LEZ claimant.
    ///
    ///      DELIBERATE ASYMMETRY — do not read "they coexist" as a feature.
    ///      This contract admits unbounded concurrent locks per hashlock; the
    ///      LEZ side does NOT. `LezClient::lock` derives the escrow PDA from
    ///      the hashlock ALONE (src/lez/client.rs:313) and refuses to fund a
    ///      hashlock whose PDA already exists, so there is exactly one
    ///      legitimate LEZ escrow per hashlock. Two ETH locks sharing a
    ///      hashlock therefore contend for a single LEZ escrow, and only one
    ///      can ever be honoured. That is a hazard the off-chain maker must
    ///      defend against (one hashlock per swap, never reused), not
    ///      something the ETH side can or should enforce.
    function test_lock_distinctSwapIdsForDifferentTakerLezAccount() public {
        vm.prank(taker);
        bytes32 swapId1 = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, TAKER_LEZ);

        vm.prank(taker);
        bytes32 swapId2 = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, TAKER_LEZ_ALT);

        // Same sender, recipient, amount, hashlock and timelock ...
        EthHTLC.HTLC memory h1 = htlc.getHTLC(swapId1);
        EthHTLC.HTLC memory h2 = htlc.getHTLC(swapId2);
        assertEq(h1.sender, h2.sender);
        assertEq(h1.recipient, h2.recipient);
        assertEq(h1.amount, h2.amount);
        assertEq(h1.hashlock, h2.hashlock);
        assertEq(h1.timelock, h2.timelock);

        // ... but distinct ids, distinct live entries, distinct LEZ claimants.
        assertTrue(swapId1 != swapId2);
        assertEq(h1.takerLezAccount, TAKER_LEZ);
        assertEq(h2.takerLezAccount, TAKER_LEZ_ALT);
        assertEq(uint8(h1.state), uint8(EthHTLC.SwapState.OPEN));
        assertEq(uint8(h2.state), uint8(EthHTLC.SwapState.OPEN));
    }

    function test_lock_takerLezAccountRoundTripsThroughGetHTLC() public {
        bytes32 swapId = _lockDefault();
        assertEq(htlc.getHTLC(swapId).takerLezAccount, TAKER_LEZ);
    }

    function test_lock_revertsWithZeroTakerLezAccount() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidTakerLezAccount.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, bytes32(0));
    }

    /// @dev Samples the "different LEZ account => different swapId" direction
    ///      only. This does NOT prove the packed encoding is injective, and no
    ///      test here does: injectivity follows from every preimage component
    ///      being fixed-width (20+20+32+32+32+32 = 168 bytes, no length-varying
    ///      field, so no boundary ambiguity), which is an argument about the
    ///      encoding, not something fuzzing can establish. What this catches is
    ///      a regression that drops takerLezAccount back out of the preimage.
    function testFuzz_lock_swapIdBindsTakerLezAccount(bytes32 lezA, bytes32 lezB) public {
        vm.assume(lezA != bytes32(0) && lezB != bytes32(0));
        vm.assume(lezA != lezB);

        vm.prank(taker);
        bytes32 idA = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, lezA);
        vm.prank(taker);
        bytes32 idB = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, lezB);

        assertTrue(idA != idB);
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
        assertEq(h.takerLezAccount, TAKER_LEZ);
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
        htlc.lock{value: 0}(HASHLOCK, TIMELOCK, maker, TAKER_LEZ);
    }

    function test_lock_revertsWithPastTimelock() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidTimelock.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, block.timestamp, maker, TAKER_LEZ);
    }

    function test_lock_revertsWithInsufficientTimelockDelta() public {
        uint256 delta = htlc.minTimelockDelta();
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidTimelock.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, block.timestamp + delta, maker, TAKER_LEZ);
    }

    function test_lock_revertsWithZeroHashlock() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidHashLock.selector);
        htlc.lock{value: AMOUNT}(bytes32(0), TIMELOCK, maker, TAKER_LEZ);
    }

    function test_lock_revertsWithZeroRecipient() public {
        vm.prank(taker);
        vm.expectRevert(EthHTLC.InvalidRecipient.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, payable(address(0)), TAKER_LEZ);
    }

    function test_lock_revertsOnDuplicate() public {
        _lockDefault();

        vm.deal(taker, 10 ether);
        vm.prank(taker);
        vm.expectRevert(EthHTLC.SwapAlreadyExists.selector);
        htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, TAKER_LEZ);
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
        bytes32 swapId = htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, badRecipient, TAKER_LEZ);

        vm.prank(badRecipient);
        vm.expectRevert(EthHTLC.TransferFailed.selector);
        htlc.claim(swapId, PREIMAGE);
    }

    function test_refund_revertsWhenSenderRejectsETH() public {
        RejectETH rejector = new RejectETH();
        address payable badSender = payable(address(rejector));
        vm.deal(badSender, 10 ether);

        vm.prank(badSender);
        htlc.lock{value: AMOUNT}(HASHLOCK, TIMELOCK, maker, TAKER_LEZ);

        bytes32 swapId = keccak256(abi.encodePacked(badSender, maker, AMOUNT, HASHLOCK, TIMELOCK, TAKER_LEZ));

        vm.warp(TIMELOCK);
        vm.prank(badSender);
        vm.expectRevert(EthHTLC.TransferFailed.selector);
        htlc.refund(swapId);
    }

    // -------------------------------------------------------------------------
    // Shared test vectors with the LEZ HTLC
    // -------------------------------------------------------------------------
    // These constants must match the Rust test suite in
    // programs/lez-htlc/methods/guest/src/main.rs (mod tests).
    // If either side changes SHA-256 behavior, the vector test below breaks.
    //
    // Only test_crossChain_sha256Compatibility is genuinely cross-chain (it
    // asserts a constant the Rust suite asserts too). The other two run the
    // Ethereum leg alone and are named accordingly.

    bytes32 constant XCHAIN_PREIMAGE = "secret_preimage_for_testing_1234";
    bytes32 constant XCHAIN_HASHLOCK = 0x0ef69611a91e0805079387fee0b89fb7d6fcd505220d407bacaa40ce031745df;

    function test_crossChain_sha256Compatibility() public pure {
        // Verify that Solidity's sha256 of our shared preimage matches
        // the hardcoded hashlock (same value asserted in the Rust tests).
        bytes32 computed = sha256(abi.encodePacked(XCHAIN_PREIMAGE));
        assertEq(computed, XCHAIN_HASHLOCK);
    }

    /// @dev NAME SCOPE: this is the ETHEREUM leg only, exercised with the
    ///      shared hashlock vector. It is deliberately NOT called a cross-chain
    ///      test, because nothing here crosses a chain. It does not prove that
    ///      the emitted takerLezAccount reaches `HTLCInstruction::Lock`, that
    ///      real `AccountId` byte ordering survives the round trip, or that
    ///      maker-equals-taker is rejected before funding. Those need a real
    ///      two-chain test in the Rust e2e suite; a name promising cross-chain
    ///      coverage here would only stop someone from writing it.
    function test_lockAndClaim_withSharedHashlockVector() public {
        // Ethereum leg: taker locks ETH under the maker's hashlock, maker
        // claims by revealing the preimage. On a real swap the same preimage
        // then unlocks the LEZ side — untested here.
        uint256 timelock = block.timestamp + 600;

        vm.prank(taker);
        bytes32 swapId = htlc.lock{value: AMOUNT}(XCHAIN_HASHLOCK, timelock, maker, TAKER_LEZ);

        // Maker claims by revealing the preimage (as they would after locking on LEZ)
        vm.prank(maker);
        htlc.claim(swapId, XCHAIN_PREIMAGE);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertEq(uint8(h.state), uint8(EthHTLC.SwapState.CLAIMED));
    }

    /// @dev ETHEREUM leg only — same naming caveat as the test above. The
    ///      corresponding LEZ-side refund is not exercised anywhere here.
    function test_refundAfterTimeout_withSharedHashlockVector() public {
        // Taker locks ETH, but Maker never claims (disappeared).
        // After timelock, Taker refunds. On LEZ side, Maker also refunds.
        uint256 timelock = block.timestamp + 600;

        vm.prank(taker);
        bytes32 swapId = htlc.lock{value: AMOUNT}(XCHAIN_HASHLOCK, timelock, maker, TAKER_LEZ);

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
        bytes32 swapId2 = htlc.lock{value: 2 ether}(HASHLOCK, timelock2, maker, TAKER_LEZ);

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
        bytes32 swapId = htlc.lock{value: amount}(fuzzHashlock, fuzzTimelock, maker, TAKER_LEZ);

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
        bytes32 swapId = htlc.lock{value: amount}(HASHLOCK, fuzzTimelock, maker, TAKER_LEZ);

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
