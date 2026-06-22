//! Inbound Debouncer
//!
//! Buffers incoming messages per key (channel_id or thread_id) and flushes
//! them after a configurable timeout. This prevents a flood of rapid
//! messages from triggering multiple concurrent agent runs.
//!

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{sleep, Duration, Instant};
use tracing::{debug, warn};

use crate::channels::IncomingMessage;

/// Configuration for the inbound debouncer.
#[derive(Debug, Clone)]
pub struct InboundDebouncerConfig {
    /// Maximum number of keys to track (LRU eviction).
    pub max_tracked_keys: usize,
    /// Debounce window in milliseconds.
    pub debounce_ms: u64,
    /// Items with this prefix in their content bypass debouncing.
    pub bypass_prefixes: Vec<String>,
}

impl Default for InboundDebouncerConfig {
    fn default() -> Self {
        Self {
            max_tracked_keys: 2048,
            debounce_ms: 500,
            bypass_prefixes: vec![
                "/".to_string(), // commands
                "!".to_string(), // bot commands
            ],
        }
    }
}

/// A single debounced item.
#[derive(Debug, Clone)]
pub struct DebouncedItem {
    pub message: IncomingMessage,
    pub received_at: Instant,
}

/// A buffer for one key.
struct DebounceBuffer {
    items: Vec<DebouncedItem>,
    /// When the timer fires, this sender will be notified.
    flush_tx: mpsc::Sender<Vec<DebouncedItem>>,
    /// The in-flight timer handle.
    timer_handle: Option<tokio::task::JoinHandle<()>>,
    /// Last time this buffer saw activity. Used for true LRU eviction.
    last_activity: Instant,
}

/// Keyed inbound debouncer.
///
/// Usage:
/// ```ignore
/// let (tx, mut rx) = mpsc::channel(100);
/// let debouncer = InboundDebouncer::new(config, tx);
/// debouncer.enqueue(message).await; // may return None if absorbed
///
/// while let Some(batch) = rx.recv().await {
/// for item in batch { /* process */ }
/// }
/// ```
pub struct InboundDebouncer {
    config: InboundDebouncerConfig,
    buffers: RwLock<HashMap<String, Mutex<DebounceBuffer>>>,
    /// Sender side of the flush channel. One receiver lives outside.
    flush_tx: mpsc::Sender<Vec<DebouncedItem>>,
}

impl InboundDebouncer {
    pub fn new(
        config: InboundDebouncerConfig,
        flush_tx: mpsc::Sender<Vec<DebouncedItem>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            buffers: RwLock::new(HashMap::new()),
            flush_tx,
        })
    }

    /// Enqueue a message for debouncing.
    ///
    /// Returns `Some(message)` immediately if the message bypasses debouncing
    /// (e.g., commands starting with `/`).
    ///
    /// Returns `None` if the message is absorbed into a pending batch.
    pub async fn enqueue(self: &Arc<Self>, message: IncomingMessage) -> Option<IncomingMessage> {
        let key = Self::resolve_key(&message);
        let content = message.content.trim();

        // Bypass: commands and other non-debouncable items go straight through.
        if self.should_bypass(content) {
            debug!("Bypassing debounce for key {} (command)", key);
            return Some(message);
        }

        let mut buffers = self.buffers.write().await;

        // LRU eviction: if we're at capacity, drop the least-recently-active key.
        if buffers.len() >= self.config.max_tracked_keys && !buffers.contains_key(&key) {
            let mut oldest_key: Option<String> = None;
            let mut oldest_time: Option<Instant> = None;
            for (k, buf) in buffers.iter() {
                let guard = buf.lock().await;
                if oldest_time.map(|t| guard.last_activity < t).unwrap_or(true) {
                    oldest_time = Some(guard.last_activity);
                    oldest_key = Some(k.clone());
                }
            }
            if let Some(oldest) = oldest_key {
                warn!(
                    "Debouncer at capacity ({}), evicting least-recently-active key {}",
                    self.config.max_tracked_keys, oldest
                );
                buffers.remove(&oldest);
            }
        }

        let buffer = buffers.entry(key.clone()).or_insert_with(|| {
            let tx = self.flush_tx.clone();
            Mutex::new(DebounceBuffer {
                items: Vec::new(),
                flush_tx: tx,
                timer_handle: None,
                last_activity: Instant::now(),
            })
        });

        let mut guard = buffer.lock().await;

        guard.items.push(DebouncedItem {
            message,
            received_at: Instant::now(),
        });
        guard.last_activity = Instant::now();

        // (Re-)start the flush timer.
        if let Some(handle) = guard.timer_handle.take() {
            handle.abort();
        }

        let debounce_ms = self.config.debounce_ms;
        let flush_tx = guard.flush_tx.clone();
        let key_clone = key.clone();
        let self_arc = self.clone();

        guard.timer_handle = Some(tokio::spawn(async move {
            sleep(Duration::from_millis(debounce_ms)).await;
            // Remove the buffer and send the batch.
            let batch = {
                let mut buffers = self_arc.buffers.write().await;
                if let Some(buf) = buffers.remove(&key_clone) {
                    let guard = buf.lock().await;
                    guard.items.clone()
                } else {
                    Vec::new()
                }
            };
            if !batch.is_empty() {
                let _ = flush_tx.send(batch).await;
            }
        }));

        None // message absorbed into pending batch
    }

    /// Flush all pending items for a given key immediately.
    pub async fn flush_key(self: &Arc<Self>, key: &str) -> Vec<IncomingMessage> {
        let batch = {
            let mut buffers = self.buffers.write().await;
            if let Some(buf) = buffers.remove(key) {
                let guard = buf.lock().await;
                if let Some(handle) = &guard.timer_handle {
                    handle.abort();
                }
                guard.items.clone()
            } else {
                Vec::new()
            }
        };

        batch.into_iter().map(|item| item.message).collect()
    }

    /// Flush **all** pending buffers (useful at shutdown).
    pub async fn flush_all(self: &Arc<Self>) -> Vec<IncomingMessage> {
        let mut all = Vec::new();
        let keys: Vec<String> = {
            let buffers = self.buffers.read().await;
            buffers.keys().cloned().collect()
        };
        for key in keys {
            all.extend(self.flush_key(&key).await);
        }
        all
    }

    /// Resolve the debounce key for a message.
    ///
    /// Uses `conversation_id` as the primary key, which corresponds to
    /// channel/thread in most cases.
    fn resolve_key(message: &IncomingMessage) -> String {
        message.conversation_id.0.clone()
    }

    /// Check if a message should bypass debouncing.
    fn should_bypass(&self, content: &str) -> bool {
        for prefix in &self.config.bypass_prefixes {
            if content.starts_with(prefix) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bypass_command() {
        let (tx, _rx) = mpsc::channel(10);
        let debouncer = InboundDebouncer::new(InboundDebouncerConfig::default(), tx);

        let msg = IncomingMessage::new("u1", "s1", "/new");
        let result = debouncer.enqueue(msg.clone()).await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_absorb_and_flush() {
        let (tx, mut rx) = mpsc::channel(10);
        let config = InboundDebouncerConfig {
            debounce_ms: 50,
            ..Default::default()
        };
        let debouncer = InboundDebouncer::new(config, tx);

        let msg1 = IncomingMessage::new("u1", "s1", "hello");
        let msg2 = IncomingMessage::new("u1", "s1", "world");

        assert!(debouncer.enqueue(msg1).await.is_none());
        assert!(debouncer.enqueue(msg2).await.is_none());

        let batch = rx.recv().await.expect("should receive batch");
        assert_eq!(batch.len(), 2);
    }

    #[tokio::test]
    async fn test_flush_key_immediate() {
        let (tx, _rx) = mpsc::channel(10);
        let debouncer = InboundDebouncer::new(InboundDebouncerConfig::default(), tx);

        let msg = IncomingMessage::new("u1", "s1", "hello");
        assert!(debouncer.enqueue(msg).await.is_none());

        let flushed = debouncer.flush_key("s1").await;
        assert_eq!(flushed.len(), 1);
    }

    #[tokio::test]
    async fn test_flush_all() {
        let (tx, _rx) = mpsc::channel(10);
        let debouncer = InboundDebouncer::new(InboundDebouncerConfig::default(), tx);

        assert!(debouncer
            .enqueue(IncomingMessage::new("u1", "s1", "a"))
            .await
            .is_none());
        assert!(debouncer
            .enqueue(IncomingMessage::new("u2", "s2", "b"))
            .await
            .is_none());

        let all = debouncer.flush_all().await;
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn test_bypass_exclamation() {
        let (tx, _rx) = mpsc::channel(10);
        let debouncer = InboundDebouncer::new(InboundDebouncerConfig::default(), tx);

        let msg = IncomingMessage::new("u1", "s1", "!command");
        let result = debouncer.enqueue(msg.clone()).await;
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_flush_empty_key() {
        let (tx, _rx) = mpsc::channel(10);
        let debouncer = InboundDebouncer::new(InboundDebouncerConfig::default(), tx);

        let flushed = debouncer.flush_key("nonexistent").await;
        assert!(flushed.is_empty());
    }

    #[test]
    fn test_config_default() {
        let config = InboundDebouncerConfig::default();
        assert_eq!(config.debounce_ms, 500);
        assert_eq!(config.max_tracked_keys, 2048);
        assert!(config.bypass_prefixes.iter().any(|p| p == "/"));
        assert!(config.bypass_prefixes.iter().any(|p| p == "!"));
    }
}
