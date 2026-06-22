use std::collections::HashMap;

use uuid::Uuid;

/// Message sent over the cross-session subagent bus.
#[derive(Debug, Clone)]
pub struct BusMessage {
    /// Unique message ID
    pub id: String,
    /// Topic the message was published on
    pub topic: String,
    /// Subagent ID that published the message
    pub sender_id: String,
    /// Message payload
    pub payload: String,
    /// When the message was sent
    pub sent_at: chrono::DateTime<chrono::Utc>,
}

/// Cross-session message bus for subagents.
///
/// Allows subagents in unrelated ACP sessions to communicate via named topics.
/// The bus is intentionally not `Clone`: it owns mutable subscriber state and
/// message history, and is shared between callers via `Arc<RwLock<AcpBus>>`.
#[derive(Debug, Default)]
pub struct AcpBus {
    /// Messages per topic, oldest first.
    messages: HashMap<String, Vec<BusMessage>>,
    /// Topic subscriptions: topic -> set of subagent IDs.
    subscriptions: HashMap<String, std::collections::HashSet<String>>,
    /// Per-subagent per-topic read offsets.
    read_offsets: HashMap<(String, String), usize>,
}

/// Maximum number of messages retained per bus topic.
const BUS_MAX_MESSAGES_PER_TOPIC: usize = 1000;
/// Maximum age of a bus message before it is eligible for pruning.
const BUS_MESSAGE_TTL: chrono::Duration = chrono::Duration::hours(1);

impl AcpBus {
    /// Create an empty bus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe a subagent to a topic.
    ///
    /// The subscriber starts receiving messages published after the
    /// subscription is created.
    pub fn subscribe(&mut self, subagent_id: &str, topic: &str) {
        self.subscriptions
            .entry(topic.to_string())
            .or_default()
            .insert(subagent_id.to_string());
        self.prune_topic(topic);
        let current_len = self.messages.get(topic).map(|v| v.len()).unwrap_or(0);
        self.read_offsets
            .entry((subagent_id.to_string(), topic.to_string()))
            .or_insert(current_len);
    }

    /// Unsubscribe a subagent from a topic.
    pub fn unsubscribe(&mut self, subagent_id: &str, topic: &str) {
        if let Some(set) = self.subscriptions.get_mut(topic) {
            set.remove(subagent_id);
            if set.is_empty() {
                self.subscriptions.remove(topic);
            }
        }
        self.read_offsets
            .remove(&(subagent_id.to_string(), topic.to_string()));
    }

    /// Remove expired and excess messages from a topic, keeping subscriber
    /// offsets consistent.
    fn prune_topic(&mut self, topic: &str) {
        let Some(messages) = self.messages.get_mut(topic) else {
            return;
        };
        let now = chrono::Utc::now();
        let original_len = messages.len();
        messages.retain(|m| now.signed_duration_since(m.sent_at) <= BUS_MESSAGE_TTL);
        let ttl_removed = original_len - messages.len();

        let cap_removed = if messages.len() > BUS_MAX_MESSAGES_PER_TOPIC {
            let excess = messages.len() - BUS_MAX_MESSAGES_PER_TOPIC;
            messages.drain(0..excess);
            excess
        } else {
            0
        };

        let total_removed = ttl_removed + cap_removed;
        if total_removed > 0 {
            for offset in self.read_offsets.values_mut() {
                *offset = offset.saturating_sub(total_removed);
            }
        }

        if messages.is_empty() {
            self.messages.remove(topic);
        }
    }

    /// Publish a message to a topic.
    pub fn publish(&mut self, topic: &str, sender_id: &str, payload: &str) -> BusMessage {
        let message = BusMessage {
            id: Uuid::new_v4().to_string(),
            topic: topic.to_string(),
            sender_id: sender_id.to_string(),
            payload: payload.to_string(),
            sent_at: chrono::Utc::now(),
        };
        self.messages
            .entry(topic.to_string())
            .or_default()
            .push(message.clone());
        self.prune_topic(topic);
        message
    }

    /// Poll pending messages for a subagent on a topic.
    pub fn poll(&mut self, subagent_id: &str, topic: &str) -> Vec<BusMessage> {
        if !self
            .subscriptions
            .get(topic)
            .map(|s| s.contains(subagent_id))
            .unwrap_or(false)
        {
            return Vec::new();
        }
        self.prune_topic(topic);
        let Some(messages) = self.messages.get(topic) else {
            return Vec::new();
        };
        let offset = self
            .read_offsets
            .entry((subagent_id.to_string(), topic.to_string()))
            .or_insert(0);
        let pending: Vec<BusMessage> = messages[*offset..].to_vec();
        *offset = messages.len();
        pending
    }

    /// Poll pending messages for a subagent across all subscribed topics.
    pub fn poll_all(&mut self, subagent_id: &str) -> HashMap<String, Vec<BusMessage>> {
        let topics: Vec<String> = self
            .subscriptions
            .iter()
            .filter(|(_, subs)| subs.contains(subagent_id))
            .map(|(topic, _)| topic.clone())
            .collect();
        topics
            .into_iter()
            .map(|topic| {
                let messages = self.poll(subagent_id, &topic);
                (topic, messages)
            })
            .collect()
    }

    /// List all topics that have at least one message or subscriber.
    pub fn topics(&self) -> Vec<String> {
        let mut topics: std::collections::HashSet<String> = self.messages.keys().cloned().collect();
        topics.extend(self.subscriptions.keys().cloned());
        topics.into_iter().collect()
    }

    /// List subscribers for a topic.
    pub fn subscribers(&self, topic: &str) -> Vec<String> {
        self.subscriptions
            .get(topic)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }
}
