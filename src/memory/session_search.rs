//! Session Search - Full-text search across conversation history
//!
//! This module implements FTS5-based search for session history,
//! allowing the agent to recall information from past conversations.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use tracing::{debug, info, warn};

/// A search result from session history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// The conversation ID
    pub conversation_id: String,
    /// The message ID
    pub message_id: String,
    /// The user who sent the message
    pub user_id: String,
    /// The message content
    pub content: String,
    /// When the message was sent
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// Search relevance score
    pub score: f64,
    /// Context (surrounding messages)
    pub context: Vec<String>,
}

/// Search query options
#[derive(Debug, Clone)]
pub struct SessionSearchQuery {
    /// The search query
    pub query: String,
    /// Filter by user ID
    pub user_id: Option<String>,
    /// Filter by conversation ID
    pub conversation_id: Option<String>,
    /// Maximum results to return
    pub limit: usize,
    /// Number of context lines before/after
    pub context_lines: usize,
    /// Minimum score threshold
    pub min_score: f64,
}

impl Default for SessionSearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            user_id: None,
            conversation_id: None,
            limit: 10,
            context_lines: 2,
            min_score: 0.0,
        }
    }
}

impl SessionSearchQuery {
    /// Create a new search query
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Default::default()
        }
    }

    /// Filter by user
    pub fn for_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Filter by conversation
    pub fn for_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    /// Set result limit
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Set context lines
    pub fn with_context(mut self, lines: usize) -> Self {
        self.context_lines = lines;
        self
    }
}

/// Session search implementation using SQLite FTS5
#[derive(Debug, Clone)]
pub struct SessionSearch {
    /// Database pool
    pool: SqlitePool,
}

impl SessionSearch {
    /// Create a new session search instance
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Initialize the FTS5 tables
    pub async fn initialize(&self) -> crate::Result<()> {
        // Create the main messages table if not exists
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                content TEXT NOT NULL,
                role TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (conversation_id) REFERENCES conversations(id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create messages table".to_string(),
            details: e.to_string(),
        })?;

        // Create the FTS5 virtual table
        sqlx::query(
            r#"
            CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
                content,
                conversation_id UNINDEXED,
                user_id UNINDEXED,
                message_id UNINDEXED,
                content_rowid=rowid
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create FTS5 table".to_string(),
            details: e.to_string(),
        })?;

        // Create triggers to keep FTS index in sync
        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS messages_fts_insert
            AFTER INSERT ON messages
            BEGIN
                INSERT INTO messages_fts(content, conversation_id, user_id, message_id)
                VALUES (new.content, new.conversation_id, new.user_id, new.id);
            END
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create insert trigger".to_string(),
            details: e.to_string(),
        })?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS messages_fts_delete
            AFTER DELETE ON messages
            BEGIN
                DELETE FROM messages_fts WHERE message_id = old.id;
            END
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create delete trigger".to_string(),
            details: e.to_string(),
        })?;

        sqlx::query(
            r#"
            CREATE TRIGGER IF NOT EXISTS messages_fts_update
            AFTER UPDATE ON messages
            BEGIN
                DELETE FROM messages_fts WHERE message_id = old.id;
                INSERT INTO messages_fts(content, conversation_id, user_id, message_id)
                VALUES (new.content, new.conversation_id, new.user_id, new.id);
            END
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to create update trigger".to_string(),
            details: e.to_string(),
        })?;

        info!("Session search FTS5 tables initialized");
        Ok(())
    }

    /// Index a new message
    pub async fn index_message(
        &self,
        message_id: impl Into<String>,
        conversation_id: impl Into<String>,
        user_id: impl Into<String>,
        content: impl Into<String>,
        role: impl Into<String>,
    ) -> crate::Result<()> {
        let message_id = message_id.into();
        let conversation_id = conversation_id.into();
        let user_id = user_id.into();
        let content = content.into();
        let role = role.into();

        sqlx::query(
            r#"
            INSERT INTO messages (id, conversation_id, user_id, content, role)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(&message_id)
        .bind(&conversation_id)
        .bind(&user_id)
        .bind(&content)
        .bind(&role)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to index message".to_string(),
            details: e.to_string(),
        })?;

        debug!("Indexed message {} in conversation {}", message_id, conversation_id);
        Ok(())
    }

    /// Search sessions
    pub async fn search(&self, query: SessionSearchQuery) -> crate::Result<Vec<SearchResult>> {
        let query_str = query.query.trim();
        if query_str.is_empty() {
            return Ok(Vec::new());
        }

        // Build the FTS5 query
        let fts_query = query_str.split_whitespace().collect::<Vec<_>>().join(" OR ");

        let rows = match (&query.user_id, &query.conversation_id) {
            (Some(user), Some(conv)) => {
                sqlx::query(
                    r#"
                    SELECT
                        m.id as message_id,
                        m.conversation_id,
                        m.user_id,
                        m.content,
                        m.created_at,
                        rank as score
                    FROM messages_fts
                    JOIN messages m ON messages_fts.message_id = m.id
                    WHERE messages_fts MATCH ?1
                        AND m.user_id = ?2
                        AND m.conversation_id = ?3
                    ORDER BY rank
                    LIMIT ?4
                    "#,
                )
                .bind(&fts_query)
                .bind(user)
                .bind(conv)
                .bind(query.limit as i64)
                .fetch_all(&self.pool)
                .await
            }
            (Some(user), None) => {
                sqlx::query(
                    r#"
                    SELECT
                        m.id as message_id,
                        m.conversation_id,
                        m.user_id,
                        m.content,
                        m.created_at,
                        rank as score
                    FROM messages_fts
                    JOIN messages m ON messages_fts.message_id = m.id
                    WHERE messages_fts MATCH ?1
                        AND m.user_id = ?2
                    ORDER BY rank
                    LIMIT ?3
                    "#,
                )
                .bind(&fts_query)
                .bind(user)
                .bind(query.limit as i64)
                .fetch_all(&self.pool)
                .await
            }
            (None, Some(conv)) => {
                sqlx::query(
                    r#"
                    SELECT
                        m.id as message_id,
                        m.conversation_id,
                        m.user_id,
                        m.content,
                        m.created_at,
                        rank as score
                    FROM messages_fts
                    JOIN messages m ON messages_fts.message_id = m.id
                    WHERE messages_fts MATCH ?1
                        AND m.conversation_id = ?2
                    ORDER BY rank
                    LIMIT ?3
                    "#,
                )
                .bind(&fts_query)
                .bind(conv)
                .bind(query.limit as i64)
                .fetch_all(&self.pool)
                .await
            }
            (None, None) => {
                sqlx::query(
                    r#"
                    SELECT
                        m.id as message_id,
                        m.conversation_id,
                        m.user_id,
                        m.content,
                        m.created_at,
                        rank as score
                    FROM messages_fts
                    JOIN messages m ON messages_fts.message_id = m.id
                    WHERE messages_fts MATCH ?1
                    ORDER BY rank
                    LIMIT ?2
                    "#,
                )
                .bind(&fts_query)
                .bind(query.limit as i64)
                .fetch_all(&self.pool)
                .await
            }
        }
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Search query failed".to_string(),
            details: e.to_string(),
        })?;

        let mut results = Vec::new();

        for row in rows {
            let score: f64 = row.try_get("score").unwrap_or(0.0);

            if score < query.min_score {
                continue;
            }

            let conversation_id: String = row.try_get("conversation_id").unwrap_or_default();
            let message_id: String = row.try_get("message_id").unwrap_or_default();

            // Get context if requested
            let context = if query.context_lines > 0 {
                self.get_context(&conversation_id, &message_id, query.context_lines)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

            results.push(SearchResult {
                conversation_id: conversation_id.clone(),
                message_id: message_id.clone(),
                user_id: row.try_get("user_id").unwrap_or_default(),
                content: row.try_get("content").unwrap_or_default(),
                timestamp: row
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                    .unwrap_or_else(|_| chrono::Utc::now()),
                score,
                context,
            });
        }

        info!(
            "Search for '{}' returned {} results",
            query.query,
            results.len()
        );
        Ok(results)
    }

    /// Get context (surrounding messages) for a message
    async fn get_context(
        &self,
        conversation_id: &str,
        message_id: &str,
        lines: usize,
    ) -> crate::Result<Vec<String>> {
        // Get the rowid of the target message
        let row: (i64,) = sqlx::query_as(
            "SELECT rowid FROM messages WHERE id = ?1 AND conversation_id = ?2"
        )
        .bind(message_id)
        .bind(conversation_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get message rowid".to_string(),
            details: e.to_string(),
        })?;

        let rowid = row.0;

        // Get context before
        let before: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT content FROM messages
            WHERE conversation_id = ?1 AND rowid < ?2
            ORDER BY rowid DESC
            LIMIT ?3
            "#
        )
        .bind(conversation_id)
        .bind(rowid)
        .bind(lines as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get context before".to_string(),
            details: e.to_string(),
        })?;

        // Get context after
        let after: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT content FROM messages
            WHERE conversation_id = ?1 AND rowid > ?2
            ORDER BY rowid ASC
            LIMIT ?3
            "#
        )
        .bind(conversation_id)
        .bind(rowid)
        .bind(lines as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| crate::error::MantaError::Storage {
            context: "Failed to get context after".to_string(),
            details: e.to_string(),
        })?;

        let mut context = before;
        context.reverse();
        context.extend(after);

        Ok(context)
    }

    /// Get message statistics
    pub async fn stats(&self) -> crate::Result<SessionStats> {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);

        let conversations: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT conversation_id) FROM messages"
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages_fts")
            .fetch_one(&self.pool)
            .await
            .unwrap_or(0);

        Ok(SessionStats {
            total_messages: total as usize,
            conversations: conversations as usize,
            indexed_messages: indexed as usize,
        })
    }

    /// Delete old messages (cleanup)
    pub async fn cleanup_before(
        &self,
        before: chrono::DateTime<chrono::Utc>,
    ) -> crate::Result<usize> {
        let result = sqlx::query("DELETE FROM messages WHERE created_at < ?1")
            .bind(before)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::error::MantaError::Storage {
                context: "Failed to cleanup old messages".to_string(),
                details: e.to_string(),
            })?;

        let deleted = result.rows_affected();
        info!("Cleaned up {} old messages", deleted);
        Ok(deleted as usize)
    }
}

/// Statistics about indexed sessions
#[derive(Debug, Clone)]
pub struct SessionStats {
    /// Total number of messages
    pub total_messages: usize,
    /// Number of conversations
    pub conversations: usize,
    /// Number of indexed messages
    pub indexed_messages: usize,
}

/// Tool for session search
pub mod tool {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolExecutionResult};
    use async_trait::async_trait;
    use serde_json::json;

    /// Tool for searching conversation history
    #[derive(Debug, Clone)]
    pub struct SessionSearchTool {
        search: SessionSearch,
    }

    impl SessionSearchTool {
        /// Create a new session search tool
        pub fn new(search: SessionSearch) -> Self {
            Self { search }
        }
    }

    #[async_trait]
    impl Tool for SessionSearchTool {
        fn name(&self) -> &str {
            "session_search"
        }

        fn description(&self) -> &str {
            r#"Search through past conversation history.

Use this when the user references past conversations, asks about previous topics,
or when you need to recall information from earlier in the conversation.

The search uses full-text search and returns the most relevant messages
from all indexed conversations."#
        }

        fn parameters_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query (keywords to search for)"
                    },
                    "conversation_id": {
                        "type": "string",
                        "description": "Optional: limit search to specific conversation"
                    },
                    "limit": {
                        "type": "integer",
                        "default": 5,
                        "description": "Maximum number of results"
                    }
                },
                "required": ["query"]
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            _context: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            let query = args["query"]
                .as_str()
                .ok_or_else(|| crate::error::MantaError::Validation(
                    "query is required".to_string()
                ))?;

            let limit = args["limit"].as_u64().unwrap_or(5) as usize;

            let search_query = SessionSearchQuery::new(query)
                .limit(limit);

            let results = self.search.search(search_query).await?;

            if results.is_empty() {
                return Ok(ToolExecutionResult::success(
                    "No matching conversations found."
                ));
            }

            let formatted: Vec<String> = results
                .iter()
                .map(|r| {
                    format!(
                        "[{}] {}: {} (score: {:.2})",
                        r.timestamp.format("%Y-%m-%d %H:%M"),
                        r.user_id,
                        &r.content[..r.content.len().min(100)],
                        r.score
                    )
                })
                .collect();

            let data = json!({
                "results": results.iter().map(|r| {
                    json!({
                        "conversation_id": r.conversation_id,
                        "message_id": r.message_id,
                        "content": r.content,
                        "user_id": r.user_id,
                        "timestamp": r.timestamp.to_rfc3339(),
                        "score": r.score
                    })
                }).collect::<Vec<_>>(),
                "count": results.len()
            });

            Ok(ToolExecutionResult::success(formatted.join("\n"))
                .with_data(data))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_query_builder() {
        let query = SessionSearchQuery::new("test query")
            .for_user("user1")
            .for_conversation("conv1")
            .limit(5);

        assert_eq!(query.query, "test query");
        assert_eq!(query.user_id, Some("user1".to_string()));
        assert_eq!(query.conversation_id, Some("conv1".to_string()));
        assert_eq!(query.limit, 5);
    }
}
