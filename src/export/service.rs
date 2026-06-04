//! Export service implementation
//!
//! Provides high-level export operations for conversations and memories.
//! Works directly with the DatabaseStore to read data and write to files.

use std::collections::HashMap;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info};

use crate::error::{Result, SyscityError};
use crate::export::formats::{
    ConversationExport, ExportFormat, ExportMeta, JsonLineMemory, JsonLineMessage, MemoryExport,
};
use crate::memory::{ChatHistoryStore, MemoryQuery, MemoryStore, UnifiedStore};
use sqlx::Row;

/// Export options for controlling export behavior
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Export format
    pub format: ExportFormat,
    /// Include embeddings in export (can significantly increase file size)
    pub include_embeddings: bool,
    /// Limit number of records (None = no limit)
    pub limit: Option<usize>,
    /// Filter by user ID (None = all users)
    pub user_id: Option<String>,
    /// Filter by conversation ID (None = all conversations)
    pub conversation_id: Option<String>,
    /// Filter by memory type (None = all types)
    pub memory_type: Option<String>,
    /// Pretty-print JSON (larger files, human-readable)
    pub pretty: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jsonl,
            include_embeddings: false,
            limit: None,
            user_id: None,
            conversation_id: None,
            memory_type: None,
            pretty: false,
        }
    }
}

impl ExportOptions {
    /// Create new export options with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set export format
    pub fn format(mut self, format: ExportFormat) -> Self {
        self.format = format;
        self
    }

    /// Include embeddings in export
    pub fn with_embeddings(mut self) -> Self {
        self.include_embeddings = true;
        self
    }

    /// Set record limit
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Filter by user ID
    pub fn for_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Filter by conversation ID
    pub fn for_conversation(mut self, conversation_id: impl Into<String>) -> Self {
        self.conversation_id = Some(conversation_id.into());
        self
    }

    /// Filter by memory type
    pub fn of_type(mut self, memory_type: impl Into<String>) -> Self {
        self.memory_type = Some(memory_type.into());
        self
    }

    /// Enable pretty-printing for JSON output
    pub fn pretty(mut self) -> Self {
        self.pretty = true;
        self
    }
}

/// Export statistics
#[derive(Debug, Clone, Default)]
pub struct ExportStats {
    /// Number of conversations exported
    pub conversation_count: usize,
    /// Number of messages exported
    pub message_count: usize,
    /// Number of memories exported
    pub memory_count: usize,
    /// Total bytes written
    pub bytes_written: u64,
}

/// Export service for generating exports from the database
pub struct ExportService {
    store: UnifiedStore,
}

impl ExportService {
    /// Create a new export service with the given store
    pub fn new(store: UnifiedStore) -> Self {
        Self { store }
    }

    /// Export conversations to a file
    ///
    /// # Arguments
    /// * `output_path` - Path to write the export file
    /// * `options` - Export options
    pub async fn export_conversations(
        &self,
        output_path: impl AsRef<Path>,
        options: &ExportOptions,
    ) -> Result<ExportStats> {
        let output_path = output_path.as_ref();
        info!(
            "Exporting conversations to {} (format: {})",
            output_path.display(),
            options.format
        );

        // Ensure parent directory exists
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SyscityError::Storage {
                    context: format!("Failed to create output directory: {}", parent.display()),
                    details: e.to_string(),
                })?;
        }

        let conversation_ids = self.get_conversation_ids(options).await?;
        let mut stats = ExportStats {
            conversation_count: conversation_ids.len(),
            ..Default::default()
        };

        match options.format {
            ExportFormat::Markdown => {
                self.write_conversations_markdown(output_path, &conversation_ids, &mut stats)
                    .await?;
            }
            ExportFormat::Json => {
                self.write_conversations_json(output_path, &conversation_ids, options, &mut stats)
                    .await?;
            }
            ExportFormat::Jsonl => {
                self.write_conversations_jsonl(output_path, &conversation_ids, options, &mut stats)
                    .await?;
            }
        }

        info!(
            "Exported {} conversations, {} messages to {} ({} bytes)",
            stats.conversation_count,
            stats.message_count,
            output_path.display(),
            stats.bytes_written
        );

        Ok(stats)
    }

    /// Export memories to a file
    ///
    /// # Arguments
    /// * `output_path` - Path to write the export file
    /// * `options` - Export options
    pub async fn export_memories(
        &self,
        output_path: impl AsRef<Path>,
        options: &ExportOptions,
    ) -> Result<ExportStats> {
        let output_path = output_path.as_ref();
        info!("Exporting memories to {} (format: {})", output_path.display(), options.format);

        // Ensure parent directory exists
        if let Some(parent) = output_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SyscityError::Storage {
                    context: format!("Failed to create output directory: {}", parent.display()),
                    details: e.to_string(),
                })?;
        }

        let memories = self.get_memories(options).await?;
        let mut stats = ExportStats {
            memory_count: memories.len(),
            ..Default::default()
        };

        match options.format {
            ExportFormat::Json => {
                self.write_memories_json(output_path, &memories, options, &mut stats)
                    .await?;
            }
            ExportFormat::Jsonl => {
                self.write_memories_jsonl(output_path, &memories, options, &mut stats)
                    .await?;
            }
            ExportFormat::Markdown => {
                // Markdown doesn't make much sense for memories, but we can do it
                self.write_memories_markdown(output_path, &memories, &mut stats)
                    .await?;
            }
        }

        info!(
            "Exported {} memories to {} ({} bytes)",
            stats.memory_count,
            output_path.display(),
            stats.bytes_written
        );

        Ok(stats)
    }

    /// Export everything (conversations + memories) to a directory
    ///
    /// Creates:
    /// - `{dir}/conversations.{format}` - All conversations
    /// - `{dir}/memories.{format}` - All memories
    /// - `{dir}/export.json` - Export metadata
    pub async fn export_all(
        &self,
        output_dir: impl AsRef<Path>,
        options: &ExportOptions,
    ) -> Result<ExportStats> {
        let output_dir = output_dir.as_ref();
        info!("Running full export to {}", output_dir.display());

        tokio::fs::create_dir_all(output_dir)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create output directory: {}", output_dir.display()),
                details: e.to_string(),
            })?;

        let ext = options.format.extension();

        // Export conversations
        let conversations_path = output_dir.join(format!("conversations.{}", ext));
        let conv_stats = self
            .export_conversations(&conversations_path, options)
            .await?;

        // Export memories
        let memories_path = output_dir.join(format!("memories.{}", ext));
        let mem_stats = self.export_memories(&memories_path, options).await?;

        // Write metadata file
        let meta_path = output_dir.join("export.json");
        let meta = ExportMeta::new();
        let meta_json = serde_json::to_string_pretty(&meta)?;

        let mut meta_file = File::create(&meta_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create metadata file: {}", meta_path.display()),
                details: e.to_string(),
            })?;
        meta_file
            .write_all(meta_json.as_bytes())
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to write metadata file: {}", meta_path.display()),
                details: e.to_string(),
            })?;

        let total_stats = ExportStats {
            conversation_count: conv_stats.conversation_count,
            message_count: conv_stats.message_count,
            memory_count: mem_stats.memory_count,
            bytes_written: conv_stats.bytes_written
                + mem_stats.bytes_written
                + meta_json.len() as u64,
        };

        info!(
            "Full export complete: {} conversations, {} messages, {} memories",
            total_stats.conversation_count, total_stats.message_count, total_stats.memory_count
        );

        Ok(total_stats)
    }

    // -------------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------------

    /// Get list of conversation IDs matching the filter options
    async fn get_conversation_ids(&self, options: &ExportOptions) -> Result<Vec<String>> {
        if let Some(ref conv_id) = options.conversation_id {
            // Specific conversation requested
            return Ok(vec![conv_id.clone()]);
        }

        if let Some(ref user_id) = options.user_id {
            // Get all conversations for this user
            let limit = options.limit.unwrap_or(1000);
            return self.store.get_user_conversations(user_id, limit).await;
        }

        // No filter - we need to get all conversations
        // This requires querying all distinct conversation IDs
        debug!("No user/conversation filter, querying all conversations");

        // Query the database directly for all conversation IDs
        let rows = sqlx::query(
            "SELECT DISTINCT conversation_id FROM chat_messages ORDER BY created_at DESC",
        )
        .fetch_all(self.store.pool())
        .await
        .map_err(|e| SyscityError::Storage {
            context: "Failed to get all conversation IDs".to_string(),
            details: e.to_string(),
        })?;

        let ids: Vec<String> = rows
            .iter()
            .filter_map(|row| row.try_get::<String, _>("conversation_id").ok())
            .collect();

        if let Some(limit) = options.limit {
            Ok(ids.into_iter().take(limit).collect())
        } else {
            Ok(ids)
        }
    }

    /// Get memories matching the filter options
    async fn get_memories(&self, options: &ExportOptions) -> Result<Vec<crate::memory::Memory>> {
        let mut query = MemoryQuery::new();

        if let Some(ref user_id) = options.user_id {
            query = query.for_user(user_id);
        }
        if let Some(ref conv_id) = options.conversation_id {
            query = query.for_conversation(conv_id);
        }
        if let Some(ref mem_type) = options.memory_type {
            query = query.of_type(mem_type);
        }

        let limit = options.limit.unwrap_or(10000);
        query = query.limit(limit);

        self.store.search(query).await
    }

    /// Write conversations in Markdown format
    async fn write_conversations_markdown(
        &self,
        output_path: &Path,
        conversation_ids: &[String],
        stats: &mut ExportStats,
    ) -> Result<()> {
        let mut file = File::create(output_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create output file: {}", output_path.display()),
                details: e.to_string(),
            })?;

        // Write header
        let header = format!(
            "# Conversation Export\n\nGenerated: {}\n\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        file.write_all(header.as_bytes())
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to write header".to_string(),
                details: e.to_string(),
            })?;
        stats.bytes_written += header.len() as u64;

        // Write each conversation
        for conv_id in conversation_ids {
            let messages = self.store.get_conversation_history(conv_id, 10000).await?;
            if messages.is_empty() {
                continue;
            }

            stats.message_count += messages.len();

            // Conversation header
            let conv_header = format!("## Conversation: {}\n\n", conv_id);
            file.write_all(conv_header.as_bytes())
                .await
                .map_err(|e| SyscityError::Storage {
                    context: format!("Failed to write conversation header: {}", conv_id),
                    details: e.to_string(),
                })?;
            stats.bytes_written += conv_header.len() as u64;

            // Messages
            for msg in messages {
                let timestamp: chrono::DateTime<chrono::Utc> = msg.created_at.into();
                let content = format!(
                    "**{}** ({})\n\n{}\n\n",
                    msg.role,
                    timestamp.format("%Y-%m-%d %H:%M:%S"),
                    msg.content
                );
                file.write_all(content.as_bytes())
                    .await
                    .map_err(|e| SyscityError::Storage {
                        context: format!("Failed to write message: {}", msg.id),
                        details: e.to_string(),
                    })?;
                stats.bytes_written += content.len() as u64;
            }

            // Separator between conversations
            let separator = "---\n\n";
            file.write_all(separator.as_bytes())
                .await
                .map_err(|e| SyscityError::Storage {
                    context: "Failed to write separator".to_string(),
                    details: e.to_string(),
                })?;
            stats.bytes_written += separator.len() as u64;
        }

        file.flush().await.map_err(|e| SyscityError::Storage {
            context: "Failed to flush file".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// Write conversations in JSON format
    async fn write_conversations_json(
        &self,
        output_path: &Path,
        conversation_ids: &[String],
        options: &ExportOptions,
        stats: &mut ExportStats,
    ) -> Result<()> {
        let mut all_messages = Vec::new();

        for conv_id in conversation_ids {
            let messages = self.store.get_conversation_history(conv_id, 10000).await?;
            stats.message_count += messages.len();
            for msg in messages {
                all_messages.push(JsonLineMessage::from_chat_message(&msg));
            }
        }

        let export = ConversationExport {
            meta: ExportMeta::new(),
            messages: all_messages,
        };

        let json = if options.pretty {
            serde_json::to_string_pretty(&export)?
        } else {
            serde_json::to_string(&export)?
        };

        let mut file = File::create(output_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create output file: {}", output_path.display()),
                details: e.to_string(),
            })?;

        file.write_all(json.as_bytes())
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to write JSON".to_string(),
                details: e.to_string(),
            })?;

        file.flush().await.map_err(|e| SyscityError::Storage {
            context: "Failed to flush file".to_string(),
            details: e.to_string(),
        })?;

        stats.bytes_written = json.len() as u64;
        Ok(())
    }

    /// Write conversations in JSONL format
    async fn write_conversations_jsonl(
        &self,
        output_path: &Path,
        conversation_ids: &[String],
        _options: &ExportOptions,
        stats: &mut ExportStats,
    ) -> Result<()> {
        let mut file = File::create(output_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create output file: {}", output_path.display()),
                details: e.to_string(),
            })?;

        for conv_id in conversation_ids {
            let messages = self.store.get_conversation_history(conv_id, 10000).await?;
            stats.message_count += messages.len();

            for msg in messages {
                let json_line = JsonLineMessage::from_chat_message(&msg);
                let line = serde_json::to_string(&json_line)?;

                file.write_all(line.as_bytes())
                    .await
                    .map_err(|e| SyscityError::Storage {
                        context: "Failed to write JSON line".to_string(),
                        details: e.to_string(),
                    })?;
                file.write_all(b"\n")
                    .await
                    .map_err(|e| SyscityError::Storage {
                        context: "Failed to write newline".to_string(),
                        details: e.to_string(),
                    })?;

                stats.bytes_written += line.len() as u64 + 1;
            }
        }

        file.flush().await.map_err(|e| SyscityError::Storage {
            context: "Failed to flush file".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// Write memories in JSON format
    async fn write_memories_json(
        &self,
        output_path: &Path,
        memories: &[crate::memory::Memory],
        options: &ExportOptions,
        stats: &mut ExportStats,
    ) -> Result<()> {
        let json_memories: Vec<JsonLineMemory> = memories
            .iter()
            .map(|m| JsonLineMemory::from_memory(m, options.include_embeddings))
            .collect();

        let export = MemoryExport {
            meta: ExportMeta::new(),
            memories: json_memories,
        };

        let json = if options.pretty {
            serde_json::to_string_pretty(&export)?
        } else {
            serde_json::to_string(&export)?
        };

        let mut file = File::create(output_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create output file: {}", output_path.display()),
                details: e.to_string(),
            })?;

        file.write_all(json.as_bytes())
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to write JSON".to_string(),
                details: e.to_string(),
            })?;

        file.flush().await.map_err(|e| SyscityError::Storage {
            context: "Failed to flush file".to_string(),
            details: e.to_string(),
        })?;

        stats.bytes_written = json.len() as u64;
        Ok(())
    }

    /// Write memories in JSONL format
    async fn write_memories_jsonl(
        &self,
        output_path: &Path,
        memories: &[crate::memory::Memory],
        options: &ExportOptions,
        stats: &mut ExportStats,
    ) -> Result<()> {
        let mut file = File::create(output_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create output file: {}", output_path.display()),
                details: e.to_string(),
            })?;

        for memory in memories {
            let json_line = JsonLineMemory::from_memory(memory, options.include_embeddings);
            let line = serde_json::to_string(&json_line)?;

            file.write_all(line.as_bytes())
                .await
                .map_err(|e| SyscityError::Storage {
                    context: "Failed to write JSON line".to_string(),
                    details: e.to_string(),
                })?;
            file.write_all(b"\n")
                .await
                .map_err(|e| SyscityError::Storage {
                    context: "Failed to write newline".to_string(),
                    details: e.to_string(),
                })?;

            stats.bytes_written += line.len() as u64 + 1;
        }

        file.flush().await.map_err(|e| SyscityError::Storage {
            context: "Failed to flush file".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }

    /// Write memories in Markdown format (for reference/documentation)
    async fn write_memories_markdown(
        &self,
        output_path: &Path,
        memories: &[crate::memory::Memory],
        stats: &mut ExportStats,
    ) -> Result<()> {
        let mut file = File::create(output_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to create output file: {}", output_path.display()),
                details: e.to_string(),
            })?;

        // Write header
        let header = format!(
            "# Memory Export\n\nGenerated: {}\n\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        file.write_all(header.as_bytes())
            .await
            .map_err(|e| SyscityError::Storage {
                context: "Failed to write header".to_string(),
                details: e.to_string(),
            })?;
        stats.bytes_written += header.len() as u64;

        // Group memories by type
        let mut by_type: HashMap<String, Vec<&crate::memory::Memory>> = HashMap::new();
        for memory in memories {
            by_type
                .entry(memory.memory_type.clone())
                .or_default()
                .push(memory);
        }

        // Write each type section
        for (mem_type, type_memories) in by_type {
            let section = format!("## {} ({} memories)\n\n", mem_type, type_memories.len());
            file.write_all(section.as_bytes())
                .await
                .map_err(|e| SyscityError::Storage {
                    context: format!("Failed to write section header: {}", mem_type),
                    details: e.to_string(),
                })?;
            stats.bytes_written += section.len() as u64;

            for memory in type_memories {
                let timestamp: chrono::DateTime<chrono::Utc> = memory.created_at.into();
                let content = format!(
                    "- **{}** (importance: {:.2}, source: {}, created: {})\n  {}\n\n",
                    memory.id,
                    memory.importance_score,
                    memory.source,
                    timestamp.format("%Y-%m-%d"),
                    memory.content.lines().collect::<Vec<_>>().join("\n  ")
                );
                file.write_all(content.as_bytes())
                    .await
                    .map_err(|e| SyscityError::Storage {
                        context: format!("Failed to write memory: {}", memory.id),
                        details: e.to_string(),
                    })?;
                stats.bytes_written += content.len() as u64;
            }
        }

        file.flush().await.map_err(|e| SyscityError::Storage {
            context: "Failed to flush file".to_string(),
            details: e.to_string(),
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_export_options_builder() {
        let opts = ExportOptions::new()
            .format(ExportFormat::Jsonl)
            .for_user("user1")
            .limit(100)
            .with_embeddings()
            .pretty();

        assert_eq!(opts.format, ExportFormat::Jsonl);
        assert_eq!(opts.user_id, Some("user1".to_string()));
        assert_eq!(opts.limit, Some(100));
        assert!(opts.include_embeddings);
        assert!(opts.pretty);
    }

    #[test]
    fn test_export_stats() {
        let stats = ExportStats {
            conversation_count: 5,
            message_count: 100,
            memory_count: 50,
            bytes_written: 1024,
        };

        assert_eq!(stats.conversation_count, 5);
        assert_eq!(stats.memory_count, 50);
    }

    #[test]
    fn test_export_options_default() {
        let opts = ExportOptions::default();
        assert_eq!(opts.format, ExportFormat::Jsonl);
        assert!(!opts.include_embeddings);
        assert_eq!(opts.limit, None);
        assert_eq!(opts.user_id, None);
        assert_eq!(opts.conversation_id, None);
        assert_eq!(opts.memory_type, None);
        assert!(!opts.pretty);
    }

    #[test]
    fn test_export_options_new_is_default() {
        let opts1 = ExportOptions::new();
        let opts2 = ExportOptions::default();
        assert_eq!(opts1.format, opts2.format);
        assert_eq!(opts1.include_embeddings, opts2.include_embeddings);
        assert_eq!(opts1.limit, opts2.limit);
    }

    #[test]
    fn test_export_options_format() {
        let opts = ExportOptions::new().format(ExportFormat::Json);
        assert_eq!(opts.format, ExportFormat::Json);
    }

    #[test]
    fn test_export_options_for_conversation() {
        let opts = ExportOptions::new().for_conversation("conv1");
        assert_eq!(opts.conversation_id, Some("conv1".to_string()));
    }

    #[test]
    fn test_export_options_of_type() {
        let opts = ExportOptions::new().of_type("fact");
        assert_eq!(opts.memory_type, Some("fact".to_string()));
    }

    #[test]
    fn test_export_options_with_embeddings() {
        let opts = ExportOptions::new().with_embeddings();
        assert!(opts.include_embeddings);
    }

    #[test]
    fn test_export_options_pretty() {
        let opts = ExportOptions::new().pretty();
        assert!(opts.pretty);
    }

    #[test]
    fn test_export_options_limit() {
        let opts = ExportOptions::new().limit(50);
        assert_eq!(opts.limit, Some(50));
    }

    #[test]
    fn test_export_options_for_user() {
        let opts = ExportOptions::new().for_user("user1");
        assert_eq!(opts.user_id, Some("user1".to_string()));
    }

    #[test]
    fn test_export_stats_default() {
        let stats: ExportStats = Default::default();
        assert_eq!(stats.conversation_count, 0);
        assert_eq!(stats.message_count, 0);
        assert_eq!(stats.memory_count, 0);
        assert_eq!(stats.bytes_written, 0);
    }

    #[test]
    fn test_export_stats_debug() {
        let stats = ExportStats {
            conversation_count: 1,
            message_count: 10,
            memory_count: 5,
            bytes_written: 100,
        };
        let debug = format!("{:?}", stats);
        assert!(debug.contains("ExportStats"));
    }
}
