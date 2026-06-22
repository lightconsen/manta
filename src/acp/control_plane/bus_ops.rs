use std::collections::HashMap;

use tracing::info;

use crate::acp::bus::BusMessage;
use super::AcpControlPlane;

impl AcpControlPlane {
    /// Subscribe a subagent to a bus topic.
    pub async fn bus_subscribe(&self, subagent_id: &str, topic: &str) -> crate::Result<()> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        bus.subscribe(subagent_id, topic);
        info!("Subagent {} subscribed to bus topic {}", subagent_id, topic);
        Ok(())
    }

    /// Unsubscribe a subagent from a bus topic.
    pub async fn bus_unsubscribe(&self, subagent_id: &str, topic: &str) {
        let mut bus = self.bus.write().await;
        bus.unsubscribe(subagent_id, topic);
        info!("Subagent {} unsubscribed from bus topic {}", subagent_id, topic);
    }

    /// Publish a message to a bus topic from a subagent.
    pub async fn bus_publish(
        &self,
        subagent_id: &str,
        topic: &str,
        payload: &str,
    ) -> crate::Result<BusMessage> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        let message = bus.publish(topic, subagent_id, payload);
        info!("Subagent {} published to bus topic {}", subagent_id, topic);
        Ok(message)
    }

    /// Poll pending bus messages for a subagent on a topic.
    pub async fn bus_poll(&self,
        subagent_id: &str,
        topic: &str,
    ) -> crate::Result<Vec<BusMessage>> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        Ok(bus.poll(subagent_id, topic))
    }

    /// Poll pending bus messages for a subagent across all subscribed topics.
    pub async fn bus_poll_all(
        &self,
        subagent_id: &str,
    ) -> crate::Result<HashMap<String, Vec<BusMessage>>> {
        {
            let subagents = self.subagents.read().await;
            if !subagents.contains_key(subagent_id) {
                return Err(crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", subagent_id),
                });
            }
        }
        let mut bus = self.bus.write().await;
        Ok(bus.poll_all(subagent_id))
    }

    /// List all bus topics.
    pub async fn bus_topics(&self) -> Vec<String> {
        let bus = self.bus.read().await;
        bus.topics()
    }

    /// List subscribers for a bus topic.
    pub async fn bus_subscribers(&self, topic: &str) -> Vec<String> {
        let bus = self.bus.read().await;
        bus.subscribers(topic)
    }
}
