//! Export service implementation
//!
//! Provides high-level export operations for conversations and memories.
//! Works directly with the DatabaseStore to read data and write to files.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use sqlx::Row;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info};

use crate::error::{Result, SyscityError};
use crate::export::formats::{
    ConversationExport, ExportFormat, ExportMeta, FullExport, JsonLineMemory, JsonLineMessage,
    MemoryExport,
};
use crate::memory::{
    ChatHistoryStore, ChatMessage, Memory, MemoryId, MemoryQuery, MemoryStore, UnifiedStore,
};

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

/// Import options for controlling import behavior
#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Skip records that already exist in the store
    pub skip_existing: bool,
    /// Overwrite existing records with the imported data
    pub update_existing: bool,
    /// Simulate the import without writing any data
    pub dry_run: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            skip_existing: false,
            update_existing: true,
            dry_run: false,
        }
    }
}

impl ImportOptions {
    /// Create new import options with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Skip existing records
    pub fn skip_existing(mut self) -> Self {
        self.skip_existing = true;
        self.update_existing = false;
        self
    }

    /// Update existing records
    pub fn update_existing(mut self) -> Self {
        self.update_existing = true;
        self.skip_existing = false;
        self
    }

    /// Run a dry-run import (no writes)
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }
}

/// Import statistics
#[derive(Debug, Clone, Default)]
pub struct ImportStats {
    /// Number of records imported as new
    pub imported: usize,
    /// Number of records skipped because they already existed
    pub skipped: usize,
    /// Number of existing records updated
    pub updated: usize,
    /// Errors encountered during import
    pub errors: Vec<String>,
}

impl ImportStats {
    /// Total number of records processed
    pub fn total(&self) -> usize {
        self.imported + self.skipped + self.updated
    }

    /// Merge another import stats into this one
    pub fn merge(&mut self, other: ImportStats) {
        self.imported += other.imported;
        self.skipped += other.skipped;
        self.updated += other.updated;
        self.errors.extend(other.errors);
    }
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

    /// Import memories from a JSON or JSONL file
    ///
    /// # Arguments
    /// * `input_path` - Path to the import file
    /// * `options` - Import options
    pub async fn import_memories(
        &self,
        input_path: impl AsRef<Path>,
        options: &ImportOptions,
    ) -> Result<ImportStats> {
        let input_path = input_path.as_ref();
        info!("Importing memories from {}", input_path.display());

        let ext = input_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jsonl")
            .to_lowercase();

        let mut stats = ImportStats::default();

        if ext == "json" {
            let content =
                tokio::fs::read_to_string(input_path)
                    .await
                    .map_err(|e| SyscityError::Storage {
                        context: format!(
                            "Failed to read memory import file: {}",
                            input_path.display()
                        ),
                        details: e.to_string(),
                    })?;

            // Try FullExport first, then MemoryExport
            if let Ok(full) = serde_json::from_str::<FullExport>(&content) {
                for json_mem in full.memories {
                    self.import_memory_record(&json_mem, options, &mut stats)
                        .await;
                }
            } else {
                let export: MemoryExport =
                    serde_json::from_str(&content).map_err(|e| SyscityError::Storage {
                        context: "Failed to parse memory JSON export".to_string(),
                        details: e.to_string(),
                    })?;
                for json_mem in export.memories {
                    self.import_memory_record(&json_mem, options, &mut stats)
                        .await;
                }
            }
        } else {
            self.import_memories_jsonl(input_path, options, &mut stats)
                .await?;
        }

        info!(
            "Memory import complete: {} imported, {} skipped, {} updated, {} errors",
            stats.imported,
            stats.skipped,
            stats.updated,
            stats.errors.len()
        );

        Ok(stats)
    }

    /// Import conversations from a JSON or JSONL file
    ///
    /// # Arguments
    /// * `input_path` - Path to the import file
    /// * `options` - Import options
    pub async fn import_conversations(
        &self,
        input_path: impl AsRef<Path>,
        options: &ImportOptions,
    ) -> Result<ImportStats> {
        let input_path = input_path.as_ref();
        info!("Importing conversations from {}", input_path.display());

        let ext = input_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jsonl")
            .to_lowercase();

        let mut stats = ImportStats::default();

        if ext == "json" {
            let content =
                tokio::fs::read_to_string(input_path)
                    .await
                    .map_err(|e| SyscityError::Storage {
                        context: format!(
                            "Failed to read conversation import file: {}",
                            input_path.display()
                        ),
                        details: e.to_string(),
                    })?;

            // Try FullExport first, then ConversationExport
            if let Ok(full) = serde_json::from_str::<FullExport>(&content) {
                for messages in full.conversations.into_values() {
                    for json_msg in messages {
                        self.import_message_record(&json_msg, options, &mut stats)
                            .await;
                    }
                }
            } else {
                let export: ConversationExport =
                    serde_json::from_str(&content).map_err(|e| SyscityError::Storage {
                        context: "Failed to parse conversation JSON export".to_string(),
                        details: e.to_string(),
                    })?;
                for json_msg in export.messages {
                    self.import_message_record(&json_msg, options, &mut stats)
                        .await;
                }
            }
        } else {
            self.import_conversations_jsonl(input_path, options, &mut stats)
                .await?;
        }

        info!(
            "Conversation import complete: {} imported, {} skipped, {} updated, {} errors",
            stats.imported,
            stats.skipped,
            stats.updated,
            stats.errors.len()
        );

        Ok(stats)
    }

    /// Import everything from an export directory
    ///
    /// Looks for `memories.{json,jsonl}` and `conversations.{json,jsonl}` in
    /// the provided directory, plus an optional `export.json` metadata
    /// file.
    pub async fn import_all(
        &self,
        input_dir: impl AsRef<Path>,
        options: &ImportOptions,
    ) -> Result<ImportStats> {
        let input_dir = input_dir.as_ref();
        info!("Running full import from {}", input_dir.display());

        if !input_dir.exists() {
            return Err(SyscityError::Storage {
                context: format!("Import directory does not exist: {}", input_dir.display()),
                details: "Directory not found".to_string(),
            });
        }

        let mut total_stats = ImportStats::default();

        if let Some(memories_path) = self.find_import_file(input_dir, "memories").await? {
            let mem_stats = self.import_memories(&memories_path, options).await?;
            total_stats.merge(mem_stats);
        }

        if let Some(conversations_path) = self.find_import_file(input_dir, "conversations").await? {
            let conv_stats = self
                .import_conversations(&conversations_path, options)
                .await?;
            total_stats.merge(conv_stats);
        }

        info!(
            "Full import complete: {} imported, {} skipped, {} updated, {} errors",
            total_stats.imported,
            total_stats.skipped,
            total_stats.updated,
            total_stats.errors.len()
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
    // -------------------------------------------------------------------------
    // Import helpers
    // -------------------------------------------------------------------------

    async fn import_memories_jsonl(
        &self,
        input_path: &Path,
        options: &ImportOptions,
        stats: &mut ImportStats,
    ) -> Result<()> {
        let file = File::open(input_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to open memory JSONL file: {}", input_path.display()),
                details: e.to_string(),
            })?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await.map_err(|e| SyscityError::Storage {
            context: "Failed to read memory JSONL line".to_string(),
            details: e.to_string(),
        })? {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JsonLineMemory>(&line) {
                Ok(json_mem) => self.import_memory_record(&json_mem, options, stats).await,
                Err(e) => stats
                    .errors
                    .push(format!("Invalid JSONL memory record: {}", e)),
            }
        }

        Ok(())
    }

    async fn import_conversations_jsonl(
        &self,
        input_path: &Path,
        options: &ImportOptions,
        stats: &mut ImportStats,
    ) -> Result<()> {
        let file = File::open(input_path)
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!(
                    "Failed to open conversation JSONL file: {}",
                    input_path.display()
                ),
                details: e.to_string(),
            })?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await.map_err(|e| SyscityError::Storage {
            context: "Failed to read conversation JSONL line".to_string(),
            details: e.to_string(),
        })? {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JsonLineMessage>(&line) {
                Ok(json_msg) => self.import_message_record(&json_msg, options, stats).await,
                Err(e) => stats
                    .errors
                    .push(format!("Invalid JSONL message record: {}", e)),
            }
        }

        Ok(())
    }

    async fn import_memory_record(
        &self,
        json: &JsonLineMemory,
        options: &ImportOptions,
        stats: &mut ImportStats,
    ) {
        let memory = match Self::json_memory_to_memory(json) {
            Ok(m) => m,
            Err(e) => {
                stats.errors.push(e);
                return;
            }
        };

        let exists = match self.memory_exists(&memory.id).await {
            Ok(e) => e,
            Err(e) => {
                stats.errors.push(e.to_string());
                return;
            }
        };

        if exists {
            if options.skip_existing {
                stats.skipped += 1;
                return;
            }
            if options.update_existing {
                if options.dry_run {
                    stats.updated += 1;
                    return;
                }
                if let Err(e) = self.store.update(memory).await {
                    stats.errors.push(e.to_string());
                } else {
                    stats.updated += 1;
                }
                return;
            }
            stats
                .errors
                .push(format!("Memory {} already exists", memory.id));
            return;
        }

        if options.dry_run {
            stats.imported += 1;
            return;
        }

        if let Err(e) = self.store.store(memory).await {
            stats.errors.push(e.to_string());
        } else {
            stats.imported += 1;
        }
    }

    async fn import_message_record(
        &self,
        json: &JsonLineMessage,
        options: &ImportOptions,
        stats: &mut ImportStats,
    ) {
        let msg = match Self::json_message_to_chat_message(json) {
            Ok(m) => m,
            Err(e) => {
                stats.errors.push(e);
                return;
            }
        };

        let exists = match self.message_exists(&msg.id).await {
            Ok(e) => e,
            Err(e) => {
                stats.errors.push(e.to_string());
                return;
            }
        };

        if exists {
            if options.skip_existing {
                stats.skipped += 1;
                return;
            }
            if options.update_existing {
                if options.dry_run {
                    stats.updated += 1;
                    return;
                }
                if let Err(e) = self.update_chat_message(&msg).await {
                    stats.errors.push(e.to_string());
                } else {
                    stats.updated += 1;
                }
                return;
            }
            stats
                .errors
                .push(format!("Message {} already exists", msg.id));
            return;
        }

        if options.dry_run {
            stats.imported += 1;
            return;
        }

        if let Err(e) = self.store.store_message(msg).await {
            stats.errors.push(e.to_string());
        } else {
            stats.imported += 1;
        }
    }

    async fn memory_exists(&self, id: &MemoryId) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM memories WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(self.store.pool())
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to check memory existence: {}", id),
                details: e.to_string(),
            })?;
        Ok(row.is_some())
    }

    async fn message_exists(&self, id: &str) -> Result<bool> {
        let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM chat_messages WHERE id = ?")
            .bind(id)
            .fetch_optional(self.store.pool())
            .await
            .map_err(|e| SyscityError::Storage {
                context: format!("Failed to check message existence: {}", id),
                details: e.to_string(),
            })?;
        Ok(row.is_some())
    }

    async fn update_chat_message(&self, msg: &ChatMessage) -> Result<()> {
        let created_at_secs = Self::system_time_to_secs(msg.created_at);
        let metadata_str = msg
            .metadata
            .as_ref()
            .map(|m| serde_json::to_string(m).unwrap_or_default());

        sqlx::query(
            "UPDATE chat_messages SET conversation_id = ?, user_id = ?, role = ?, content = ?, \
             created_at = ?, metadata = ? WHERE id = ?",
        )
        .bind(&msg.conversation_id)
        .bind(&msg.user_id)
        .bind(&msg.role)
        .bind(&msg.content)
        .bind(created_at_secs)
        .bind(metadata_str)
        .bind(&msg.id)
        .execute(self.store.pool())
        .await
        .map_err(|e| SyscityError::Storage {
            context: format!("Failed to update chat message: {}", msg.id),
            details: e.to_string(),
        })?;

        Ok(())
    }

    fn system_time_to_secs(time: SystemTime) -> i64 {
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    async fn find_import_file(&self, dir: &Path, base: &str) -> Result<Option<std::path::PathBuf>> {
        for ext in ["jsonl", "json"] {
            let path = dir.join(format!("{}.{}", base, ext));
            if path.exists() {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    fn json_memory_to_memory(json: &JsonLineMemory) -> std::result::Result<Memory, String> {
        json.validate()?;
        let created_at = Self::parse_timestamp(&json.created_at)?;
        let expires_at = json
            .expires_at
            .as_ref()
            .map(|s| Self::parse_timestamp(s))
            .transpose()?;

        Ok(Memory {
            id: MemoryId::new(&json.id),
            user_id: json.user_id.clone(),
            conversation_id: json.conversation_id.clone(),
            content: json.content.clone(),
            memory_type: json.memory_type.clone(),
            embedding: json.embedding.clone(),
            created_at,
            expires_at,
            metadata: json.metadata.clone(),
            importance_score: json.importance_score,
            source: json.source.clone(),
        })
    }

    fn json_message_to_chat_message(
        json: &JsonLineMessage,
    ) -> std::result::Result<ChatMessage, String> {
        json.validate()?;
        let created_at = Self::parse_timestamp(&json.timestamp)?;

        Ok(ChatMessage {
            id: json.id.clone(),
            conversation_id: json.conversation_id.clone(),
            user_id: json.user_id.clone(),
            role: json.role.clone(),
            content: json.content.clone(),
            created_at,
            metadata: json.metadata.clone(),
        })
    }

    fn parse_timestamp(s: &str) -> std::result::Result<SystemTime, String> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc).into())
            .map_err(|e| format!("Invalid timestamp '{}': {}", s, e))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    async fn in_memory_store() -> UnifiedStore {
        UnifiedStore::new_in_memory()
            .await
            .expect("in-memory store")
    }

    #[tokio::test]
    async fn test_import_memories_jsonl() {
        let store = in_memory_store().await;
        let memory = crate::memory::Memory::new("user1", "hello world", "fact");
        let memory_id = memory.id.clone();
        store.store(memory).await.unwrap();

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("memories.jsonl");
        let export_opts = ExportOptions::new().format(ExportFormat::Jsonl);
        let service = ExportService::new(store);
        service.export_memories(&path, &export_opts).await.unwrap();

        let target_store = in_memory_store().await;
        let target_service = ExportService::new(target_store);
        let import_opts = ImportOptions::new();
        let stats = target_service
            .import_memories(&path, &import_opts)
            .await
            .unwrap();

        assert_eq!(stats.imported, 1);
        assert_eq!(stats.errors.len(), 0);
        let imported = target_service
            .store
            .get(&MemoryId::new(memory_id.to_string()))
            .await
            .unwrap();
        assert!(imported.is_some());
        assert_eq!(imported.unwrap().content, "hello world");
    }

    #[tokio::test]
    async fn test_import_memories_skip_update_dry_run() {
        let store = in_memory_store().await;
        let memory = crate::memory::Memory::new("user1", "original", "fact");
        let memory_id = memory.id.clone();
        store.store(memory).await.unwrap();

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("memories.jsonl");
        let export_opts = ExportOptions::new().format(ExportFormat::Jsonl);
        let service = ExportService::new(store);
        service.export_memories(&path, &export_opts).await.unwrap();

        // Skip existing
        let stats = service
            .import_memories(&path, &ImportOptions::new().skip_existing())
            .await
            .unwrap();
        assert_eq!(stats.skipped, 1);

        // Dry run with default update_existing
        let stats = service
            .import_memories(&path, &ImportOptions::new().dry_run())
            .await
            .unwrap();
        assert_eq!(stats.updated, 1);

        // Update existing by mutating file content
        let mut content = tokio::fs::read_to_string(&path).await.unwrap();
        content = content.replace("original", "updated");
        let updated_path = tmp.path().join("updated.jsonl");
        tokio::fs::write(&updated_path, content).await.unwrap();

        let stats = service
            .import_memories(&updated_path, &ImportOptions::new().update_existing())
            .await
            .unwrap();
        assert_eq!(stats.updated, 1);
        let updated = service
            .store
            .get(&MemoryId::new(memory_id.to_string()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.content, "updated");
    }

    #[tokio::test]
    async fn test_import_conversations_jsonl() {
        let store = in_memory_store().await;
        let msg = crate::memory::ChatMessage::new("conv1", "user1", "user", "hi");
        let msg_id = msg.id.clone();
        store.store_message(msg).await.unwrap();

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("conversations.jsonl");
        let export_opts = ExportOptions::new().format(ExportFormat::Jsonl);
        let service = ExportService::new(store);
        service
            .export_conversations(&path, &export_opts)
            .await
            .unwrap();

        let target_store = in_memory_store().await;
        let target_service = ExportService::new(target_store);
        let stats = target_service
            .import_conversations(&path, &ImportOptions::new())
            .await
            .unwrap();

        assert_eq!(stats.imported, 1);
        let history = target_service
            .store
            .get_conversation_history("conv1", 10)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, msg_id);
    }

    #[tokio::test]
    async fn test_import_all_round_trip() {
        let store = in_memory_store().await;
        store
            .store(crate::memory::Memory::new("u1", "memory one", "fact"))
            .await
            .unwrap();
        store
            .store_message(crate::memory::ChatMessage::new("c1", "u1", "user", "hello"))
            .await
            .unwrap();

        let tmp = TempDir::new().unwrap();
        let export_dir = tmp.path().join("export");
        let service = ExportService::new(store);
        service
            .export_all(&export_dir, &ExportOptions::new().format(ExportFormat::Jsonl))
            .await
            .unwrap();

        let target_store = in_memory_store().await;
        let target_service = ExportService::new(target_store);
        let stats = target_service
            .import_all(&export_dir, &ImportOptions::new())
            .await
            .unwrap();

        assert_eq!(stats.imported, 2);
        assert_eq!(stats.errors.len(), 0);
    }

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
