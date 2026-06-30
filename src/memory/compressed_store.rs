//! Compressed JSONL Store — cold archival memory backend
//!
//! Stores memories as gzip-compressed JSONL files on disk.
//! Optimised for batch-append writes and infrequent reads.
//! Each file is a daily shard: `archival/YYYY-MM-DD.jsonl.gz`.
//!
//! # Design trade-offs
//! - Fast append (lock-free via atomic rename)
//! - Slow random access (must scan + decompress)
//! - Compact on disk (~10x vs raw JSON)
//! - Suitable for Archival tier: rarely accessed, read in bulk

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::{Memory, MemoryId, MemoryQuery, MemoryStats, MemoryStore};

/// Directory name for archival shards.
pub const ARCHIVAL_DIR_NAME: &str = "archival";

/// Gzip-compressed JSONL memory store for cold archival storage.
#[derive(Debug)]
pub struct CompressedJsonlStore {
    /// Base directory (e.g. `{workspace}/memory/archival/`)
    dir: PathBuf,
    /// Maximum memories per shard before rotating to next day.
    max_per_shard: usize,
    /// Mutex for synchronising write operations (store, update, delete, cleanup).
    /// Prevents concurrent read-modify-write cycles from silently losing data.
    write_lock: Arc<Mutex<()>>,
}

impl Clone for CompressedJsonlStore {
    fn clone(&self) -> Self {
        Self {
            dir: self.dir.clone(),
            max_per_shard: self.max_per_shard,
            write_lock: Arc::clone(&self.write_lock),
        }
    }
}

impl CompressedJsonlStore {
    /// Create a new compressed store at the given base path.
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self {
            dir: base_dir.as_ref().join(ARCHIVAL_DIR_NAME),
            max_per_shard: 10_000,
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Builder: set max memories per shard.
    pub fn with_max_per_shard(mut self, n: usize) -> Self {
        self.max_per_shard = n;
        self
    }

    /// Ensure the archival directory exists.
    async fn ensure_dir(&self) -> crate::Result<()> {
        fs::create_dir_all(&self.dir)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to create archival dir: {:?}", self.dir),
                details: e.to_string(),
            })
    }

    /// Shard path for today's date.
    fn today_shard(&self) -> PathBuf {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.dir.join(format!("{}.jsonl.gz", today))
    }

    /// List all shard files, sorted oldest first.
    async fn list_shards(&self) -> Vec<PathBuf> {
        let mut shards = Vec::new();
        let Ok(mut read_dir) = fs::read_dir(&self.dir).await else {
            return shards;
        };
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("gz") {
                shards.push(path);
            }
        }
        shards.sort();
        shards
    }

    /// Append a single memory to the current shard.
    async fn append_one(&self, memory: &Memory) -> crate::Result<()> {
        self.ensure_dir().await?;

        let shard = self.today_shard();
        let line =
            serde_json::to_string(memory).map_err(crate::error::SyscityError::Serialization)?;

        // Read existing content if file exists
        let mut existing = Vec::new();
        if shard.exists() {
            match fs::read(&shard).await {
                Ok(data) => existing = data,
                Err(e) => {
                    warn!("Failed to read archival shard {:?}, treating as empty: {}", shard, e);
                }
            }
        }

        // Decompress existing, append line, recompress
        let mut all_lines: Vec<String> = if existing.is_empty() {
            Vec::new()
        } else {
            Self::decompress_lines(&existing)?
        };

        all_lines.push(line);

        // Check shard size
        if all_lines.len() > self.max_per_shard {
            warn!(
                "Archival shard {:?} exceeded max_per_shard ({}); continuing anyway",
                shard, self.max_per_shard
            );
        }

        let compressed = Self::compress_lines(&all_lines)?;

        // Atomic write: write to temp file, then rename.
        let tmp_path = self.dir.join(format!(
            ".{}.tmp.{}",
            shard.file_name().unwrap_or_default().to_string_lossy(),
            std::process::id()
        ));
        fs::write(&tmp_path, compressed).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: format!("Failed to write temp shard: {:?}", tmp_path),
                details: e.to_string(),
            }
        })?;
        fs::rename(&tmp_path, &shard)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: format!("Failed to rename temp shard to {:?}", shard),
                details: e.to_string(),
            })?;

        Ok(())
    }

    /// Decompress gzip bytes into lines.
    fn decompress_lines(data: &[u8]) -> crate::Result<Vec<String>> {
        let decoder = flate2::read::GzDecoder::new(data);
        let reader = std::io::BufReader::new(decoder);
        let mut lines = Vec::new();
        for line in reader.lines() {
            let line = line.map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to read line from archival shard".to_string(),
                details: e.to_string(),
            })?;
            if !line.trim().is_empty() {
                lines.push(line);
            }
        }
        Ok(lines)
    }

    /// Compress lines into gzip bytes.
    fn compress_lines(lines: &[String]) -> crate::Result<Vec<u8>> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        for line in lines {
            writeln!(encoder, "{}", line).map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to write line to archival shard".to_string(),
                details: e.to_string(),
            })?;
        }
        encoder
            .finish()
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to finish gzip compression".to_string(),
                details: e.to_string(),
            })
    }

    /// Load all memories from all shards.
    async fn load_all(&self) -> crate::Result<Vec<Memory>> {
        let shards = self.list_shards().await;
        let mut memories = Vec::new();
        for shard in shards {
            let data = fs::read(&shard)
                .await
                .map_err(|e| crate::error::SyscityError::Storage {
                    context: format!("Failed to read archival shard: {:?}", shard),
                    details: e.to_string(),
                })?;
            let lines = Self::decompress_lines(&data)?;
            for line in lines {
                match serde_json::from_str::<Memory>(&line) {
                    Ok(mem) => memories.push(mem),
                    Err(e) => {
                        warn!("Skipping malformed archival entry: {}", e);
                    }
                }
            }
        }
        Ok(memories)
    }

    /// Rewrite all shards with the given memories (used by delete/update).
    async fn rewrite_all(&self, memories: Vec<Memory>) -> crate::Result<()> {
        // Group by shard date based on created_at
        let mut by_date: HashMap<String, Vec<Memory>> = HashMap::new();
        for mem in memories {
            let date = chrono::DateTime::<chrono::Utc>::from(mem.created_at)
                .format("%Y-%m-%d")
                .to_string();
            by_date.entry(date).or_default().push(mem);
        }

        // List old shards before we touch anything.
        let old_shards = self.list_shards().await;

        // Write new shards to temp files first, then rename atomically.
        let mut new_shards: Vec<PathBuf> = Vec::new();
        for (date, mems) in &by_date {
            let shard = self.dir.join(format!("{}.jsonl.gz", date));
            let lines: Vec<String> = mems
                .iter()
                .map(|m| {
                    serde_json::to_string(m).map_err(|e| {
                        warn!("Failed to serialize memory {} during rewrite: {}", m.id, e);
                        e
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::error::SyscityError::Serialization)?;
            let compressed = Self::compress_lines(&lines)?;

            let tmp_path = self.dir.join(format!(
                ".{}.tmp.{}",
                shard.file_name().unwrap_or_default().to_string_lossy(),
                std::process::id()
            ));
            fs::write(&tmp_path, compressed).await.map_err(|e| {
                crate::error::SyscityError::Storage {
                    context: format!("Failed to write temp shard: {:?}", tmp_path),
                    details: e.to_string(),
                }
            })?;
            fs::rename(&tmp_path, &shard).await.map_err(|e| {
                crate::error::SyscityError::Storage {
                    context: format!("Failed to rename temp shard to {:?}", shard),
                    details: e.to_string(),
                }
            })?;
            new_shards.push(shard);
        }

        // Remove old shards that were not rewritten.
        for old in &old_shards {
            if !new_shards.contains(old) {
                if let Err(e) = fs::remove_file(old).await {
                    warn!("Failed to remove stale archival shard {:?}: {}", old, e);
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl MemoryStore for CompressedJsonlStore {
    async fn store(&self, memory: Memory) -> crate::Result<MemoryId> {
        let id = memory.id.clone();
        let _guard = self.write_lock.lock().await;
        self.append_one(&memory).await?;
        drop(_guard);
        debug!("Archived memory: {}", id);
        Ok(id)
    }

    async fn get(&self, id: &MemoryId) -> crate::Result<Option<Memory>> {
        let all = self.load_all().await?;
        Ok(all.into_iter().find(|m| m.id == *id))
    }

    async fn update(&self, memory: Memory) -> crate::Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut all = self.load_all().await?;
        let mut found = false;
        for m in &mut all {
            if m.id == memory.id {
                *m = memory.clone();
                found = true;
                break;
            }
        }
        if !found {
            return Err(crate::error::SyscityError::NotFound {
                resource: format!("Memory {}", memory.id),
            });
        }
        self.rewrite_all(all).await
    }

    async fn delete(&self, id: &MemoryId) -> crate::Result<bool> {
        let _guard = self.write_lock.lock().await;
        let mut all = self.load_all().await?;
        let before = all.len();
        all.retain(|m| m.id != *id);
        let removed = before - all.len();
        if removed > 0 {
            self.rewrite_all(all).await?;
        }
        Ok(removed > 0)
    }

    async fn search(&self, query: MemoryQuery) -> crate::Result<Vec<Memory>> {
        let all = self.load_all().await?;
        let mut results: Vec<Memory> = all
            .into_iter()
            .filter(|m| {
                if let Some(ref user_id) = query.user_id {
                    if m.user_id != *user_id {
                        return false;
                    }
                }
                if let Some(ref conv_id) = query.conversation_id {
                    if m.conversation_id.as_ref() != Some(conv_id) {
                        return false;
                    }
                }
                if let Some(ref mem_type) = query.memory_type {
                    if m.memory_type != *mem_type {
                        return false;
                    }
                }
                if let Some(ref content) = query.content_query {
                    if !m.content.to_lowercase().contains(&content.to_lowercase()) {
                        return false;
                    }
                }
                if !query.include_expired && m.is_expired() {
                    return false;
                }
                true
            })
            .collect();

        results.sort_by(|a, b| {
            b.importance_score
                .partial_cmp(&a.importance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let offset = query.offset.min(results.len());
        let limit = query.limit.min(results.len() - offset);
        Ok(results.into_iter().skip(offset).take(limit).collect())
    }

    async fn cleanup_expired(&self) -> crate::Result<usize> {
        let _guard = self.write_lock.lock().await;
        let mut all = self.load_all().await?;
        let before = all.len();
        all.retain(|m| !m.is_expired());
        let removed = before - all.len();
        if removed > 0 {
            self.rewrite_all(all).await?;
            info!("Cleaned up {} expired archival memories", removed);
        }
        Ok(removed)
    }

    async fn stats(&self) -> crate::Result<MemoryStats> {
        let all = self.load_all().await?;
        let total = all.len();
        let mut count_by_type = HashMap::new();
        let mut expired = 0;
        for m in &all {
            *count_by_type.entry(m.memory_type.clone()).or_insert(0) += 1;
            if m.is_expired() {
                expired += 1;
            }
        }
        Ok(MemoryStats {
            total_count: total,
            count_by_type,
            expired_count: expired,
        })
    }

    async fn close(&self) -> crate::Result<()> {
        info!("CompressedJsonlStore closed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn test_compressed_store_roundtrip() {
        let dir = tempdir().unwrap();
        let store = CompressedJsonlStore::new(dir.path());

        let mem = Memory::new("u1", "Archival fact", "fact").with_importance_score(0.9);
        let id = store.store(mem.clone()).await.unwrap();

        let fetched = store.get(&id).await.unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().content, "Archival fact");
    }

    #[tokio::test]
    async fn test_compressed_search() {
        let dir = tempdir().unwrap();
        let store = CompressedJsonlStore::new(dir.path());

        store
            .store(Memory::new("u1", "Old project notes", "note"))
            .await
            .unwrap();
        store
            .store(Memory::new("u1", "Very old diary entry", "diary"))
            .await
            .unwrap();
        store
            .store(Memory::new("u2", "Another user's note", "note"))
            .await
            .unwrap();

        let results = store
            .search(MemoryQuery::new().for_user("u1").with_content("old"))
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_compressed_delete_and_rewrite() {
        let dir = tempdir().unwrap();
        let store = CompressedJsonlStore::new(dir.path());

        let mem1 = Memory::new("u1", "Keep me", "fact");
        let mem2 = Memory::new("u1", "Delete me", "fact");
        let id1 = store.store(mem1).await.unwrap();
        let id2 = store.store(mem2).await.unwrap();

        let deleted = store.delete(&id2).await.unwrap();
        assert!(deleted);

        let stats = store.stats().await.unwrap();
        assert_eq!(stats.total_count, 1);

        let remaining = store.get(&id1).await.unwrap();
        assert!(remaining.is_some());
    }
}
