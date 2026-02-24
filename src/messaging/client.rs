use base64::prelude::*;
use reqwest::Client;
use serde::{Serialize, de::DeserializeOwned};
use tracing::{debug, warn};

use crate::error::{Result, SwapError};

use super::types::{StoreMessageEntry, StoreQueryResponse, WakuRelayMessage, WakuRelayResponse};

pub struct MessagingClient {
    http: Client,
    base_url: String,
}

impl MessagingClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Subscribe to content topics so relay cache collects messages for polling.
    pub async fn subscribe(&self, topics: &[&str]) -> Result<()> {
        let url = format!("{}/relay/v1/auto/subscriptions", self.base_url);
        debug!(topics = ?topics, "subscribing to topics");

        let resp = self
            .http
            .post(&url)
            .json(&topics)
            .send()
            .await
            .map_err(|e| SwapError::Messaging(format!("subscribe request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SwapError::Messaging(format!(
                "subscribe failed ({status}): {body}"
            )));
        }

        debug!("subscribed successfully");
        Ok(())
    }

    /// Publish a message to a content topic via relay.
    pub async fn publish<T: Serialize>(&self, topic: &str, payload: &T) -> Result<()> {
        let json_bytes = serde_json::to_vec(payload)
            .map_err(|e| SwapError::Messaging(format!("failed to serialize payload: {e}")))?;

        let b64 = BASE64_STANDARD.encode(&json_bytes);

        let msg = WakuRelayMessage {
            payload: b64,
            content_topic: topic.to_string(),
            timestamp: None,
        };

        let url = format!("{}/relay/v1/auto/messages", self.base_url);
        debug!(topic, payload_len = json_bytes.len(), "publishing message");

        let resp = self
            .http
            .post(&url)
            .json(&msg)
            .send()
            .await
            .map_err(|e| SwapError::Messaging(format!("publish request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SwapError::Messaging(format!(
                "publish failed ({status}): {body}"
            )));
        }

        debug!("message published");
        Ok(())
    }

    /// Poll relay cache for messages on a topic. **Drains the cache** (nwaku
    /// clears messages on read). Returns empty vec on 404 (not subscribed).
    pub async fn poll_messages<T: DeserializeOwned>(&self, topic: &str) -> Result<Vec<T>> {
        let encoded_topic = urlencoding::encode(topic);
        let url = format!(
            "{}/relay/v1/auto/messages/{}",
            self.base_url, encoded_topic
        );

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| SwapError::Messaging(format!("poll request failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            warn!(topic, "topic not subscribed (404), returning empty");
            return Ok(vec![]);
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SwapError::Messaging(format!(
                "poll failed ({status}): {body}"
            )));
        }

        let raw_messages: Vec<WakuRelayResponse> = resp
            .json()
            .await
            .map_err(|e| SwapError::Messaging(format!("failed to parse poll response: {e}")))?;

        let mut results = Vec::new();
        for msg in raw_messages {
            match decode_payload(&msg.payload) {
                Ok(item) => results.push(item),
                Err(e) => {
                    warn!("skipping message with undecodable payload: {e}");
                }
            }
        }

        debug!(topic, count = results.len(), "polled messages");
        Ok(results)
    }

    /// Query the store for historical messages on the given topics.
    pub async fn store_query(
        &self,
        topics: &[&str],
        start_time_ns: Option<i64>,
        page_size: Option<u64>,
    ) -> Result<Vec<StoreMessageEntry>> {
        let mut url = format!(
            "{}/store/v3/messages?includeData=true",
            self.base_url
        );

        for topic in topics {
            url.push_str(&format!("&contentTopics={}", urlencoding::encode(topic)));
        }

        if let Some(ts) = start_time_ns {
            url.push_str(&format!("&startTime={ts}"));
        }

        if let Some(ps) = page_size {
            url.push_str(&format!("&pageSize={ps}"));
        }

        debug!(%url, "querying store");

        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| SwapError::Messaging(format!("store query request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SwapError::Messaging(format!(
                "store query failed ({status}): {body}"
            )));
        }

        let store_resp: StoreQueryResponse = resp
            .json()
            .await
            .map_err(|e| SwapError::Messaging(format!("failed to parse store response: {e}")))?;

        debug!(count = store_resp.messages.len(), "store query returned");
        Ok(store_resp.messages)
    }
}

/// Decode a base64 Waku payload into a typed value.
fn decode_payload<T: DeserializeOwned>(b64: &str) -> std::result::Result<T, String> {
    let bytes = BASE64_STANDARD
        .decode(b64)
        .map_err(|e| format!("base64 decode: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("json deserialize: {e}"))
}

/// Decode a base64 Waku payload (public helper for store message decoding).
pub fn decode_waku_payload<T: DeserializeOwned>(b64: &str) -> Result<T> {
    decode_payload(b64).map_err(|e| SwapError::Messaging(format!("payload decode failed: {e}")))
}
