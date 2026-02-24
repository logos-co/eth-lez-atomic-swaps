use alloy::primitives::FixedBytes;
use lez_htlc_program::{HTLCEscrow, HTLCState};
use serde_json::json;

use crate::eth::client::EthHTLC;
use crate::eth::client::EthHTLC::SwapState;
use crate::swap::refund::now_unix;
use crate::swap::types::SwapOutcome;

pub fn print_swap_outcome(outcome: &SwapOutcome, json: bool) {
    match outcome {
        SwapOutcome::Completed {
            preimage,
            eth_tx,
            lez_tx,
        } => {
            if json {
                let v = json!({
                    "status": "completed",
                    "preimage": hex::encode(preimage),
                    "eth_tx": format!("{eth_tx}"),
                    "lez_tx": lez_tx,
                });
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                println!("Swap completed!");
                println!("  preimage: {}", hex::encode(preimage));
                println!("  ETH tx:   {eth_tx}");
                println!("  LEZ tx:   {lez_tx}");
            }
        }
        SwapOutcome::Refunded {
            eth_refund_tx,
            lez_refund_tx,
        } => {
            if json {
                let v = json!({
                    "status": "refunded",
                    "eth_refund_tx": eth_refund_tx.map(|tx| format!("{tx}")),
                    "lez_refund_tx": lez_refund_tx,
                });
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            } else {
                println!("Swap refunded.");
                if let Some(tx) = eth_refund_tx {
                    println!("  ETH refund tx: {tx}");
                }
                if let Some(tx) = lez_refund_tx {
                    println!("  LEZ refund tx: {tx}");
                }
            }
        }
    }
}

pub fn print_escrow(escrow: &HTLCEscrow, json: bool) {
    let state_str = match escrow.state {
        HTLCState::Locked => "Locked",
        HTLCState::Claimed => "Claimed",
        HTLCState::Refunded => "Refunded",
    };

    if json {
        let v = json!({
            "chain": "LEZ",
            "state": state_str,
            "hashlock": hex::encode(escrow.hashlock),
            "maker_id": hex::encode(escrow.maker_id.value()),
            "taker_id": hex::encode(escrow.taker_id.value()),
            "amount": escrow.amount,
            "preimage": escrow.preimage.as_ref().map(hex::encode),
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        println!("LEZ Escrow:");
        println!("  state:    {state_str}");
        println!("  hashlock: {}", hex::encode(escrow.hashlock));
        println!("  maker:    {}", hex::encode(escrow.maker_id.value()));
        println!("  taker:    {}", hex::encode(escrow.taker_id.value()));
        println!("  amount:   {}", escrow.amount);
        if let Some(preimage) = &escrow.preimage {
            println!("  preimage: {}", hex::encode(preimage));
        }
    }
}

pub fn print_htlc(htlc: &EthHTLC::HTLC, swap_id: FixedBytes<32>, json_output: bool) {
    let state_str = match htlc.state {
        SwapState::EMPTY => "Empty",
        SwapState::OPEN => "Open",
        SwapState::CLAIMED => "Claimed",
        SwapState::REFUNDED => "Refunded",
        _ => "Unknown",
    };

    let timelock_secs: u64 = htlc.timelock.try_into().unwrap_or(u64::MAX);
    let now = now_unix();
    let remaining = timelock_secs.saturating_sub(now);

    if json_output {
        let v = json!({
            "chain": "ETH",
            "swap_id": format!("{swap_id}"),
            "state": state_str,
            "sender": format!("{}", htlc.sender),
            "recipient": format!("{}", htlc.recipient),
            "amount_wei": htlc.amount.to_string(),
            "hashlock": format!("{}", htlc.hashlock),
            "timelock": timelock_secs,
            "time_remaining_secs": remaining,
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
    } else {
        println!("ETH HTLC:");
        println!("  swap_id:   {swap_id}");
        println!("  state:     {state_str}");
        println!("  sender:    {}", htlc.sender);
        println!("  recipient: {}", htlc.recipient);
        println!("  amount:    {} wei", htlc.amount);
        println!("  hashlock:  {}", htlc.hashlock);
        println!("  timelock:  {timelock_secs} (Unix)");
        if remaining > 0 {
            let mins = remaining / 60;
            let secs = remaining % 60;
            println!("  remaining: {mins}m {secs}s");
        } else {
            println!("  remaining: expired");
        }
    }
}
