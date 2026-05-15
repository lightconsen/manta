//! Persistent Audit Log for Manta
//!
//! Provides SQLite-backed persistent storage of audit entries,
//! complementing the in-memory RuntimeAuditLog with durability.
//!
//! All security-relevant events are written to SQLite for:
//! - Forensic analysis
//! - Compliance reporting
//! - Long-term retention

use crate::security::runtime_audit::{AuditEntry, AuditEventType};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Persistent audit log backed by SQLite
#[derive(Debug, Clone)]
pub struct PersistentAuditLog {
    /// In-memory ring buffer for fast recent queries
    memory: Arc<RwLock<Vec<AuditEntry>>>,
    /// SQLite pool for persistent storage
    pool: Option<sqlx::SqlitePool>,
    /// Max in-memory entries
    memory_capacity: usize,
}

impl PersistentAuditLog {
    /// Create a new persistent audit log (in-memory only, no persistence)
    pub fn new() -> Self {
        Self {
            memory: Arc::new(RwLock::new(Vec::new())),
            pool: None,
            memory_capacity: 1000,
        }
    }

    /// Create with SQLite persistence
    pub fn with_pool(pool: sqlx::SqlitePool) -> Self {
        let s = Self {
            memory: Arc::new(RwLock::new(Vec::new())),
            pool: Some(pool),
            memory_capacity: 1000,
        };
        s
    }

    /// Initialize the audit table (call once at startup)
    pub async fn init(&self) -> crate::Result<()> {
        if let Some(ref pool) = self.pool {
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS audit_log (
                    id TEXT PRIMARY KEY,
                    timestamp REAL NOT NULL,
                    event_type TEXT NOT NULL,
                    actor TEXT NOT NULL,
                    target TEXT NOT NULL,
                    allowed INTEGER NOT NULL,
                    description TEXT NOT NULL,
                    details TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
                CREATE INDEX IF NOT EXISTS idx_audit_event_type ON audit_log(event_type);
                CREATE INDEX IF NOT EXISTS idx_audit_actor ON audit_log(actor);
                "#,
            )
            .execute(pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to create audit table".into(),
                details: e.to_string(),
            })?;
            info!("Persistent audit log table initialized");
        }
        Ok(())
    }

    /// Log a new audit entry (memory + persistent storage)
    pub async fn log(
        &self,
        event_type: AuditEventType,
        actor: impl Into<String>,
        target: impl Into<String>,
        allowed: bool,
        description: impl Into<String>,
        details: Option<serde_json::Value>,
    ) {
        let entry = AuditEntry {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: std::time::SystemTime::now(),
            event_type,
            actor: actor.into(),
            target: target.into(),
            allowed,
            description: description.into(),
            details: details.clone(),
        };

        // Store in memory
        {
            let mut mem = self.memory.write().await;
            if mem.len() >= self.memory_capacity {
                mem.remove(0);
            }
            mem.push(entry.clone());
        }

        // Persist to SQLite
        if let Some(ref pool) = self.pool {
            let details_str = details.map(|d| d.to_string());
            let timestamp_secs = entry
                .timestamp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64();

            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO audit_log (id, timestamp, event_type, actor, target, allowed, description, details)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                "#,
            )
            .bind(&entry.id)
            .bind(timestamp_secs)
            .bind(format!("{:?}", entry.event_type))
            .bind(&entry.actor)
            .bind(&entry.target)
            .bind(if entry.allowed { 1 } else { 0 })
            .bind(&entry.description)
            .bind(details_str)
            .execute(pool)
            .await
            {
                warn!("Failed to persist audit entry: {}", e);
            }
        }
    }

    /// Retrieve recent entries from memory (fast)
    pub async fn recent(&self, n: usize) -> Vec<AuditEntry> {
        let mem = self.memory.read().await;
        mem.iter().rev().take(n).cloned().collect()
    }

    /// Query persistent store for entries in a time range
    pub async fn query_range(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
        limit: i64,
    ) -> Vec<AuditEntry> {
        let mut entries = Vec::new();
        if let Some(ref pool) = self.pool {
            let start_secs = start.timestamp() as f64;
            let end_secs = end.timestamp() as f64;

            match sqlx::query(
                r#"
                SELECT id, timestamp, event_type, actor, target, allowed, description, details
                FROM audit_log
                WHERE timestamp >= ?1 AND timestamp <= ?2
                ORDER BY timestamp DESC
                LIMIT ?3
                "#,
            )
            .bind(start_secs)
            .bind(end_secs)
            .bind(limit)
            .fetch_all(pool)
            .await
            {
                Ok(rows) => {
                    for row in rows {
                        let event_type_str: String = row.get("event_type");
                        let event_type = match event_type_str.as_str() {
                            "AccessCheck" => AuditEventType::AccessCheck,
                            "PairingRequest" => AuditEventType::PairingRequest,
                            "PairingApprove" => AuditEventType::PairingApprove,
                            "PairingReject" => AuditEventType::PairingReject,
                            "PairingRevoke" => AuditEventType::PairingRevoke,
                            "CommandGate" => AuditEventType::CommandGate,
                            "ConfigChange" => AuditEventType::ConfigChange,
                            "ToolInvocation" => AuditEventType::ToolInvocation,
                            "ToolDeny" => AuditEventType::ToolDeny,
                            _ => AuditEventType::Security,
                        };

                        let details: Option<String> = row.get("details");
                        let details_json = details.and_then(|d| serde_json::from_str(&d).ok());

                        entries.push(AuditEntry {
                            id: row.get("id"),
                            timestamp: std::time::UNIX_EPOCH
                                + std::time::Duration::from_secs_f64(row.get("timestamp")),
                            event_type,
                            actor: row.get("actor"),
                            target: row.get("target"),
                            allowed: row.get::<i32, _>("allowed") != 0,
                            description: row.get("description"),
                            details: details_json,
                        });
                    }
                }
                Err(e) => {
                    warn!("Failed to query audit log: {}", e);
                }
            }
        }
        entries
    }

    /// Get total count of persisted entries
    pub async fn persisted_count(&self) -> i64 {
        if let Some(ref pool) = self.pool {
            match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM audit_log")
                .fetch_one(pool)
                .await
            {
                Ok(count) => return count,
                Err(e) => {
                    warn!("Failed to count audit entries: {}", e);
                }
            }
        }
        0
    }

    /// Get all entries from memory (oldest first)
    pub async fn all(&self) -> Vec<AuditEntry> {
        let mem = self.memory.read().await;
        mem.iter().cloned().collect()
    }

    /// Filter entries by event type
    pub async fn filter(&self, event_type: AuditEventType) -> Vec<AuditEntry> {
        let mem = self.memory.read().await;
        mem.iter().filter(|e| e.event_type == event_type).cloned().collect()
    }

    /// Current entry count in memory
    pub async fn len(&self) -> usize {
        self.memory.read().await.len()
    }

    /// Clear memory buffer
    pub async fn clear(&self) {
        self.memory.write().await.clear();
    }

    /// Export all entries as JSON
    pub async fn export_json(&self) -> Result<String, serde_json::Error> {
        let mem = self.memory.read().await;
        serde_json::to_string_pretty(&*mem)
    }
}

impl Default for PersistentAuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_audit_log() {
        let log = PersistentAuditLog::new();
        log.log(
            AuditEventType::AccessCheck,
            "user1",
            "telegram",
            true,
            "Access allowed",
            None,
        )
        .await;

        let entries = log.recent(10).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].actor, "user1");
    }
}
