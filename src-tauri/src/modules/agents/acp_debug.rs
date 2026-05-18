#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// Maximum size of the debug ring buffer in bytes (10MB).
const DEFAULT_MAX_BYTES: usize = 10 * 1024 * 1024;

/// A byte-bounded ring buffer for ACP protocol debug messages.
///
/// Captures:
/// - Sent JSON-RPC messages
/// - Received JSON-RPC responses
/// - Transport errors
/// - Protocol-level events
///
/// DEV ONLY. Not exposed in production UI.
pub struct AcpDebugBuffer {
    messages: Mutex<Vec<DebugMessage>>,
    current_bytes: Mutex<usize>,
    max_bytes: usize,
    enabled: Mutex<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugMessage {
    #[serde(with = "serde_millis")]
    pub timestamp: std::time::SystemTime,
    pub direction: MessageDirection,
    pub raw_content: String,
    pub byte_size: usize,
    pub agent_id: String,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDirection {
    Sent,
    Received,
    Error,
}

mod serde_millis {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(time: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        serializer.serialize_u64(millis)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let millis = u64::deserialize(deserializer)?;
        Ok(UNIX_EPOCH + std::time::Duration::from_millis(millis))
    }
}

impl AcpDebugBuffer {
    pub fn new(max_bytes: Option<usize>) -> Self {
        Self {
            messages: Mutex::new(Vec::with_capacity(100)),
            current_bytes: Mutex::new(0),
            max_bytes: max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
            enabled: Mutex::new(false),
        }
    }

    pub fn enable(&self) {
        *self.enabled.lock().unwrap() = true;
        log::info!("ACP debug buffer enabled (max {} bytes)", self.max_bytes);
    }

    pub fn disable(&self) {
        *self.enabled.lock().unwrap() = false;
        self.clear();
        log::info!("ACP debug buffer disabled and cleared");
    }

    pub fn is_enabled(&self) -> bool {
        *self.enabled.lock().unwrap()
    }

    /// Record a debug message.
    pub fn record(
        &self,
        direction: MessageDirection,
        raw_content: &str,
        agent_id: &str,
        session_id: Option<&str>,
    ) {
        if !self.is_enabled() {
            return;
        }

        let msg = DebugMessage {
            timestamp: std::time::SystemTime::now(),
            direction,
            raw_content: raw_content.to_string(),
            byte_size: raw_content.len(),
            agent_id: agent_id.to_string(),
            session_id: session_id.map(|s| s.to_string()),
        };

        let mut msgs = self.messages.lock().unwrap();
        let mut current = self.current_bytes.lock().unwrap();

        // Evict oldest if over limit
        while *current + msg.byte_size > self.max_bytes && !msgs.is_empty() {
            let removed = msgs.remove(0);
            *current = current.saturating_sub(removed.byte_size);
        }

        *current += msg.byte_size;
        msgs.push(msg);
    }

    /// Get all buffered messages (for the debug panel).
    pub fn get_messages(&self) -> Vec<DebugMessage> {
        self.messages.lock().unwrap().clone()
    }

    /// Clear the buffer.
    pub fn clear(&self) {
        self.messages.lock().unwrap().clear();
        *self.current_bytes.lock().unwrap() = 0;
    }

    /// Get current buffer stats.
    pub fn stats(&self) -> DebugBufferStats {
        let msgs = self.messages.lock().unwrap();
        let current = *self.current_bytes.lock().unwrap();
        DebugBufferStats {
            message_count: msgs.len(),
            byte_count: current,
            max_bytes: self.max_bytes,
            usage_percent: (current as f64 / self.max_bytes as f64) * 100.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugBufferStats {
    pub message_count: usize,
    pub byte_count: usize,
    pub max_bytes: usize,
    pub usage_percent: f64,
}

/// Redact sensitive content before storing in debug buffer.
pub fn redact_for_debug(raw: &str) -> String {
    // Redact common patterns
    let redacted = raw
        .replace(
            &regex::Regex::new(r#""api_key"\s*:\s*"[^"]+""#).unwrap().to_string(),
            r#""api_key": "REDACTED""#
        )
        .replace(
            &regex::Regex::new(r#""token"\s*:\s*"[^"]+""#).unwrap().to_string(),
            r#""token": "REDACTED""#
        );
    // Truncate to 10KB per message
    if redacted.len() > 10240 {
        format!("{}...(truncated {})", &redacted[..10240], redacted.len() - 10240)
    } else {
        redacted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_buffer_evicts_on_overflow() {
        let buffer = AcpDebugBuffer::new(Some(100)); // 100 bytes max
        buffer.enable();
        
        // Add 200 bytes worth of messages
        for _ in 0..10 {
            buffer.record(MessageDirection::Sent, &"x".repeat(20), "test", None);
        }
        
        let stats = buffer.stats();
        assert!(stats.byte_count <= 100);
        assert!(stats.message_count < 10);
    }

    #[test]
    fn debug_buffer_disabled_ignores_records() {
        let buffer = AcpDebugBuffer::new(None);
        // Not enabled
        buffer.record(MessageDirection::Sent, "test", "test", None);
        assert_eq!(buffer.get_messages().len(), 0);
    }
}
