//! Run with: cargo test --test print_config -- --nocapture
//!
//! Prints the deterministic config values for local testing.
//! Does NOT deploy anything — just computes keys and IDs.

use nssa::{AccountId, PrivateKey, PublicKey};

fn lez_key(seed: u8) -> (String, String) {
    let key = PrivateKey::try_new([seed; 32]).unwrap();
    let pub_key = PublicKey::new_from_private_key(&key);
    let id = AccountId::from(&pub_key);
    (hex::encode([seed; 32]), hex::encode(id.as_ref()))
}

fn program_id_hex() -> String {
    let id: [u32; 8] = lez_htlc_methods::LEZ_HTLC_PROGRAM_ID;
    let bytes: Vec<u8> = id.iter().flat_map(|w| w.to_le_bytes()).collect();
    hex::encode(bytes)
}

#[test]
fn print_local_config() {
    let (maker_key, maker_id) = lez_key(1);
    let (taker_key, taker_id) = lez_key(2);
    let program_id = program_id_hex();

    println!("\n========== LEZ Config Values ==========");
    println!("LEZ_SIGNING_KEY={maker_key}");
    println!("LEZ_HTLC_PROGRAM_ID={program_id}");
    println!("LEZ_TAKER_ACCOUNT_ID={taker_id}");
    println!();
    println!("# Maker account ID (for sequencer initial_accounts):");
    println!("#   {maker_id}");
    println!("# Taker account ID (for sequencer initial_accounts):");
    println!("#   {taker_id}");
    println!();
    println!("# Taker signing key (if running taker flow):");
    println!("#   LEZ_SIGNING_KEY={taker_key}");
    println!("=========================================\n");
}
