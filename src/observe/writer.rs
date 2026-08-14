//! Persists turn observability records to disk.
//!
//! Layout: `<base>/YYYY-MM-DD/<turn_id>.json` (local-time date partition).
//! Writes are atomic (tmp file + rename). Both an async and a blocking
//! variant are provided — the blocking one backs the collector's `Drop`
//! fallback where no async context may exist.

use std::path::{Path, PathBuf};

use crate::error::{Result, SyscityError};

use super::record::TurnRecord;

pub struct TurnMetricsWriter {
    base: PathBuf,
}

impl TurnMetricsWriter {
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    /// Default writer rooted at `~/.syscity/turns`.
    pub fn default_dir() -> Self {
        Self::new(crate::dirs::turns_dir())
    }

    fn target_path(&self, turn_id: &str) -> PathBuf {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        self.base.join(date).join(format!("{}.json", turn_id))
    }

    fn serialize(rec: &TurnRecord) -> Result<String> {
        let mut rec = rec.clone();
        rec.finalize();
        serde_json::to_string_pretty(&rec)
            .map_err(|e| SyscityError::Internal(format!("Failed to serialize turn record: {}", e)))
    }

    /// Write the record asynchronously.
    pub async fn write(&self, rec: &TurnRecord) -> Result<()> {
        let target = self.target_path(&rec.turn_id);
        let json = Self::serialize(rec)?;
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = target.with_extension("json.tmp");
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, &target).await?;
        Ok(())
    }

    /// Blocking write for contexts without an async runtime (Drop fallback).
    pub fn write_blocking(&self, rec: &TurnRecord) -> Result<()> {
        let target = self.target_path(&rec.turn_id);
        let json = Self::serialize(rec)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = target.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    }

    pub fn base(&self) -> &Path {
        &self.base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::record::{ObservedUsage, TurnEndState};
    use tempfile::TempDir;

    fn sample_record(id: &str) -> TurnRecord {
        TurnRecord {
            schema_version: 1,
            turn_id: id.into(),
            session_id: Some("s".into()),
            conversation_id: "c".into(),
            agent_id: "worker".into(),
            thread_id: "main".into(),
            turn_index: 0,
            state: TurnEndState::Complete,
            started_at: "2026-08-14T10:00:00+08:00".into(),
            finished_at: "2026-08-14T10:00:01+08:00".into(),
            duration_ms: 1000,
            ttft_ms: None,
            model: "m".into(),
            user_message_preview: "u".into(),
            assistant_text_preview: "a".into(),
            reasoning_preview: String::new(),
            queue_wait_ms: None,
            cache_hit: false,
            error: None,
            usage: ObservedUsage::default(),
            llm_rounds: vec![],
            tool_calls: vec![],
        }
    }

    #[tokio::test]
    async fn write_creates_date_dir_and_file() {
        let dir = TempDir::new().unwrap();
        let writer = TurnMetricsWriter::new(dir.path().to_path_buf());
        writer.write(&sample_record("t1")).await.unwrap();

        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let file = dir.path().join(date).join("t1.json");
        assert!(file.exists());
        let content = std::fs::read_to_string(&file).unwrap();
        let parsed: TurnRecord = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.turn_id, "t1");
        // No tmp file left behind
        assert!(!dir
            .path()
            .join(chrono::Local::now().format("%Y-%m-%d").to_string())
            .join("t1.json.tmp")
            .exists());
    }

    #[tokio::test]
    async fn concurrent_writes_both_land() {
        let dir = TempDir::new().unwrap();
        let writer = TurnMetricsWriter::new(dir.path().to_path_buf());
        let (r1, r2) = (sample_record("a"), sample_record("b"));
        let (res1, res2) = tokio::join!(writer.write(&r1), writer.write(&r2));
        res1.unwrap();
        res2.unwrap();

        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert!(dir.path().join(&date).join("a.json").exists());
        assert!(dir.path().join(&date).join("b.json").exists());
    }

    #[test]
    fn blocking_write_works_without_runtime() {
        let dir = TempDir::new().unwrap();
        let writer = TurnMetricsWriter::new(dir.path().to_path_buf());
        writer.write_blocking(&sample_record("t2")).unwrap();
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert!(dir.path().join(date).join("t2.json").exists());
    }
}
