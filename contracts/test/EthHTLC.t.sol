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
        htlc = new EthHTLC();
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
        assertTrue(h.claimed);
        assertFalse(h.refunded);
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
        assertFalse(h.claimed);
        assertFalse(h.refunded);
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
        assertTrue(h.refunded);
        assertFalse(h.claimed);
    }

    function test_refund_succeedsAtExactTimelock() public {
        bytes32 swapId = _lockDefault();

        vm.warp(TIMELOCK);

        vm.prank(taker);
        htlc.refund(swapId);

        EthHTLC.HTLC memory h = htlc.getHTLC(swapId);
        assertTrue(h.refunded);
    }

    function test_refund_revertsBeforeTimelock() public {
        bytes32 swapId = _lockDefault();

        vm.prank(taker);
        vm.expectRevert(EthHTLC.TimelockNotExpired.selector);
        htlc.refund(swapId);
    }
}
