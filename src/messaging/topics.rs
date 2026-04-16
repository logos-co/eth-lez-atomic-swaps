//! Pubsub & content-topic constants and helpers.
//!
//! `waku-bindings` requires an explicit `PubsubTopic` parameter on
//! `relay_subscribe` / `relay_publish_message` — it does not auto-shard
//! from the content topic the way nwaku's `/relay/v1/auto/...` REST
//! endpoints do. So every node in our mesh subscribes to the same
//! pubsub topic (`/waku/2/rs/0/0`) and routes by content topic
//! application-side.
//!
//! See `delivery-dogfooding.md` entry #1 for context.

use std::borrow::Cow;

use waku_bindings::node::PubsubTopic;
use waku_bindings::{Encoding, WakuContentTopic};

/// The single pubsub topic used by every node in the mesh.
/// Cluster `0`, shard `0` — minimum-viable configuration for local dev.
pub const PUBSUB_TOPIC_STR: &str = "/waku/2/rs/0/0";

/// Content topic for broadcast offers (maker → taker discovery).
pub const OFFERS_TOPIC: &str = "/atomic-swaps/1/offers/json";

/// Content topic for per-swap coordination (keyed by hashlock).
pub fn swap_topic(hashlock: &[u8; 32]) -> String {
    format!("/atomic-swaps/1/swap-{}/json", hex::encode(hashlock))
}

/// Build the canonical `PubsubTopic` value for the mesh.
pub fn pubsub_topic() -> PubsubTopic {
    PubsubTopic::new(PUBSUB_TOPIC_STR)
}

/// Convert a content-topic string (`/app/ver/name/encoding`) into
/// `WakuContentTopic`. Used at the publish boundary where we accept
/// `&str` topics from callers.
pub fn parse_content_topic(s: &str) -> WakuContentTopic {
    // Direct struct construction avoids `WakuContentTopic::new`'s
    // `&'static str` requirement, which would otherwise force callers
    // to leak runtime strings.
    if let Some(parts) = parse_topic_parts(s) {
        let (app, ver, name, enc) = parts;
        WakuContentTopic {
            application_name: Cow::Owned(app),
            version: Cow::Owned(ver),
            content_topic_name: Cow::Owned(name),
            encoding: enc,
        }
    } else {
        // Fallback: treat the whole string as the content topic name
        // under the swap namespace. Should never happen for our topics
        // but keeps the API total.
        WakuContentTopic {
            application_name: Cow::Borrowed("atomic-swaps"),
            version: Cow::Borrowed("1"),
            content_topic_name: Cow::Owned(s.to_string()),
            encoding: Encoding::Unknown("json".to_string()),
        }
    }
}

fn parse_topic_parts(s: &str) -> Option<(String, String, String, Encoding)> {
    let trimmed = s.strip_prefix('/')?;
    let parts: Vec<&str> = trimmed.splitn(4, '/').collect();
    if parts.len() != 4 {
        return None;
    }
    let encoding = match parts[3] {
        "proto" => Encoding::Proto,
        "rlp" => Encoding::Rlp,
        "rfc26" => Encoding::Rfc26,
        other => Encoding::Unknown(other.to_string()),
    };
    Some((
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].to_string(),
        encoding,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_offers_topic() {
        let t = parse_content_topic(OFFERS_TOPIC);
        assert_eq!(t.application_name, "atomic-swaps");
        assert_eq!(t.version, "1");
        assert_eq!(t.content_topic_name, "offers");
        assert_eq!(t.encoding, Encoding::Unknown("json".to_string()));
        assert_eq!(t.to_string(), OFFERS_TOPIC);
    }

    #[test]
    fn parses_swap_topic() {
        let hashlock = [0xABu8; 32];
        let s = swap_topic(&hashlock);
        let t = parse_content_topic(&s);
        assert_eq!(t.application_name, "atomic-swaps");
        assert_eq!(t.content_topic_name, format!("swap-{}", hex::encode(hashlock)));
        assert_eq!(t.to_string(), s);
    }
}
