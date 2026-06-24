//! Trajectory Log
//!
//! Captures the execution trace of an agent turn: tool calls,
//! reasoning steps, provider latencies, and other observability data.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::info;

use crate::error::Result;

#[allow(clippy::unwrap_used)]
static RE_HOME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(~|/Users/\w+|/home/\w+)").unwrap());
#[allow(clippy::unwrap_used)]
static RE_HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9a-fA-F]{32,}").unwrap());

/// A single entry in the trajectory log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TrajectoryEntry {
    /// The agent started processing.
    Start {
        timestamp: SystemTime,
        session_id: String,
        agent_id: String,
    },
    /// A tool was invoked.
    ToolCall {
        timestamp: SystemTime,
        name: String,
        arguments: serde_json::Value,
    },
    /// A tool returned a result.
    ToolResult {
        timestamp: SystemTime,
        name: String,
        result: serde_json::Value,
        duration_ms: u64,
    },
    /// The LLM provider was called.
    LlmCall {
        timestamp: SystemTime,
        provider: String,
        model: String,
        input_tokens: Option<u32>,
        output_tokens: Option<u32>,
        duration_ms: u64,
    },
    /// A reasoning or planning step.
    Reasoning {
        timestamp: SystemTime,
        step: String,
        detail: String,
    },
    /// The agent finished.
    Finish {
        timestamp: SystemTime,
        output: String,
    },
    /// An error occurred.
    Error {
        timestamp: SystemTime,
        message: String,
    },
}

/// The full trajectory for a single agent turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrajectoryLog {
    pub entries: Vec<TrajectoryEntry>,
}

impl TrajectoryLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, entry: TrajectoryEntry) {
        self.entries.push(entry);
    }

    pub fn tool_calls(&self) -> Vec<&TrajectoryEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e, TrajectoryEntry::ToolCall { .. }))
            .collect()
    }

    pub fn llm_calls(&self) -> Vec<&TrajectoryEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e, TrajectoryEntry::LlmCall { .. }))
            .collect()
    }

    pub fn total_duration_ms(&self) -> u64 {
        let start = self.entries.iter().find_map(|e| match e {
            TrajectoryEntry::Start { timestamp, .. } => Some(*timestamp),
            _ => None,
        });
        let end = self.entries.iter().find_map(|e| match e {
            TrajectoryEntry::Finish { timestamp, .. } => Some(*timestamp),
            _ => None,
        });
        match (start, end) {
            (Some(s), Some(e)) => e.duration_since(s).unwrap_or_default().as_millis() as u64,
            _ => 0,
        }
    }
}

/// Default max file size: 512 MB.
const DEFAULT_MAX_FILE_SIZE: u64 = 512 * 1024 * 1024;

/// Default max event size: 256 KB.
const DEFAULT_MAX_EVENT_SIZE: usize = 256 * 1024;

/// Persists trajectory entries to disk as JSONL files.
///
/// Each session writes entries into a dated file under
/// `~/.syscity/trajectory/`. Files are rotated when they exceed
/// `max_file_size`. Individual events are truncated at `max_event_size`.
pub struct TrajectoryWriter {
    base_dir: PathBuf,
    current_file: tokio::sync::Mutex<Option<(String, tokio::io::BufWriter<tokio::fs::File>)>>,
    current_file_size: Arc<AtomicU64>,
    max_file_size: u64,
    max_event_size: usize,
}

impl TrajectoryWriter {
    /// Create a new writer with defaults (base dir: `~/.syscity/trajectory/`,
    /// max file 512 MB, max event 256 KB).
    pub fn new() -> Self {
        Self {
            base_dir: trajectory_dir(),
            current_file: tokio::sync::Mutex::new(None),
            current_file_size: Arc::new(AtomicU64::new(0)),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_event_size: DEFAULT_MAX_EVENT_SIZE,
        }
    }

    /// Override the base directory.
    pub fn with_dir(mut self, dir: PathBuf) -> Self {
        self.base_dir = dir;
        self
    }

    /// Append a single entry to the current file for the given session.
    ///
    /// Creates a new file when none is open, the session changes, or the
    /// current file exceeds `max_file_size`.
    pub async fn append(&self, session_id: &str, entry: &TrajectoryEntry) -> Result<()> {
        let json = serde_json::to_string(entry)?;

        let json = if json.len() > self.max_event_size {
            json.chars().take(self.max_event_size).collect::<String>()
        } else {
            json
        };

        let line = format!("{json}\n");
        let line_len = line.len() as u64;

        let mut guard = self.current_file.lock().await;

        // Rotate file if needed.
        let needs_new = match &*guard {
            None => true,
            Some((current_session, _)) => {
                current_session != session_id
                    || self.current_file_size.load(Ordering::Relaxed) + line_len
                        > self.max_file_size
            }
        };

        if needs_new {
            // Dropping the old BufWriter flushes & closes the file.
            *guard = None;

            let filename = generate_filename(session_id);
            tokio::fs::create_dir_all(&self.base_dir).await?;
            let path = self.base_dir.join(&filename);
            let file = tokio::fs::File::create(&path).await?;
            let writer = tokio::io::BufWriter::new(file);
            info!(path = %path.display(), "trajectory: opened new file");
            *guard = Some((session_id.to_string(), writer));
            self.current_file_size.store(0, Ordering::Relaxed);
        }

        if let Some((_, ref mut writer)) = &mut *guard {
            writer.write_all(line.as_bytes()).await?;
            writer.flush().await?;
            self.current_file_size
                .fetch_add(line_len, Ordering::Relaxed);
        }

        Ok(())
    }

    /// Write all entries from a `TrajectoryLog` for the given session.
    pub async fn append_log(&self, session_id: &str, log: &TrajectoryLog) -> Result<()> {
        for entry in &log.entries {
            self.append(session_id, entry).await?;
        }
        Ok(())
    }

    /// Collect all JSONL files for a session, redact paths, and write a
    /// single export JSON file to `~/.syscity/trajectory/exports/`.
    ///
    /// Returns the path to the exported file.
    pub async fn export_bundle(&self, session_id: &str) -> Result<PathBuf> {
        let files = self.session_files(session_id).await?;
        let mut all_text = Vec::new();

        for file_path in &files {
            let content = tokio::fs::read_to_string(file_path).await?;
            for line in content.lines() {
                if !line.trim().is_empty() {
                    let redacted = Self::redact(line);
                    all_text.push(redacted);
                }
            }
        }

        // Write as a JSON array of (redacted) JSON objects.
        let export_dir = self.base_dir.join("exports");
        tokio::fs::create_dir_all(&export_dir).await?;
        let export_path = export_dir.join(format!("{}-export.json", slugify(session_id)));

        let bundle = serde_json::to_string_pretty(&all_text)?;
        tokio::fs::write(&export_path, bundle).await?;

        Ok(export_path)
    }

    /// Read all entries for a session from disk and reconstruct a
    /// `TrajectoryLog`.
    pub async fn get_trajectory(&self, session_id: &str) -> Result<TrajectoryLog> {
        let files = self.session_files(session_id).await?;
        let mut log = TrajectoryLog::new();

        for file_path in &files {
            let content = tokio::fs::read_to_string(file_path).await?;
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    let entry: TrajectoryEntry = serde_json::from_str(trimmed)?;
                    log.push(entry);
                }
            }
        }

        Ok(log)
    }

    /// List unique session IDs found in the trajectory directory.
    pub async fn list_sessions(&self) -> Result<Vec<String>> {
        let mut sessions: Vec<String> = Vec::new();

        let mut entries = match tokio::fs::read_dir(&self.base_dir).await {
            Ok(d) => d,
            Err(_) => return Ok(sessions), // directory doesn't exist yet
        };

        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("trajectory-") && name_str.ends_with(".jsonl") {
                // Extract session_id slug from: trajectory-YYYY-MM-DD-HHMMSS-slug.jsonl
                // The slug is everything after the last '-' in the stem.
                let stem = name_str
                    .strip_prefix("trajectory-")
                    .and_then(|s| s.strip_suffix(".jsonl"))
                    .unwrap_or("");
                if let Some(slug) = stem.rsplit('-').next() {
                    if !slug.is_empty() && !sessions.contains(&slug.to_string()) {
                        sessions.push(slug.to_string());
                    }
                }
            }
        }

        sessions.sort();
        Ok(sessions)
    }

    /// Redact potentially sensitive paths and tokens from a string.
    ///
    /// - Replaces `~`, `/Users/<name>`, `/home/<name>` with `$HOME`
    /// - Replaces 32+ character hex strings with `[REDACTED]`
    pub fn redact(input: &str) -> String {
        let result = RE_HOME.replace_all(input, "$$HOME");
        let result = RE_HEX.replace_all(&result, "[REDACTED]");
        result.to_string()
    }

    // ---- internal helpers ----

    /// Return all JSONL file paths in the base dir that belong to a session.
    async fn session_files(&self, session_id: &str) -> Result<Vec<PathBuf>> {
        let slug = slugify(session_id);
        let mut files: Vec<PathBuf> = Vec::new();

        let mut entries = match tokio::fs::read_dir(&self.base_dir).await {
            Ok(d) => d,
            Err(_) => return Ok(files),
        };

        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            if name_str.starts_with("trajectory-")
                && name_str.ends_with(&format!("-{}.jsonl", slug))
            {
                files.push(entry.path());
            }
        }

        files.sort();
        Ok(files)
    }
}

impl Default for TrajectoryWriter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Returns the default trajectory directory: `~/.syscity/trajectory/`.
pub fn trajectory_dir() -> PathBuf {
    crate::dirs::syscity_dir().join("trajectory")
}

/// Generate a JSONL filename for a session, based on the current timestamp.
///
/// Format: `trajectory-YYYY-MM-DD-HHMMSS-<slug>.jsonl`
fn generate_filename(session_id: &str) -> String {
    let now = chrono::Local::now();
    let slug = slugify(session_id);
    format!("trajectory-{}-{}.jsonl", now.format("%Y-%m-%d-%H%M%S"), slug)
}

/// Convert a string to a safe filesystem slug: alphanumeric + hyphens only,
/// maximum 40 characters.
fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .take(40)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trajectory_push() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Start {
            timestamp: SystemTime::now(),
            session_id: "s1".to_string(),
            agent_id: "a1".to_string(),
        });
        assert_eq!(log.entries.len(), 1);
    }

    #[test]
    fn test_tool_calls_filter() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Start {
            timestamp: SystemTime::now(),
            session_id: "s1".to_string(),
            agent_id: "a1".to_string(),
        });
        log.push(TrajectoryEntry::ToolCall {
            timestamp: SystemTime::now(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "rust"}),
        });
        assert_eq!(log.tool_calls().len(), 1);
    }

    #[test]
    fn test_llm_calls_filter() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::LlmCall {
            timestamp: SystemTime::now(),
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            input_tokens: Some(10),
            output_tokens: Some(20),
            duration_ms: 500,
        });
        log.push(TrajectoryEntry::ToolCall {
            timestamp: SystemTime::now(),
            name: "search".to_string(),
            arguments: serde_json::json!({}),
        });
        assert_eq!(log.llm_calls().len(), 1);
    }

    #[test]
    fn test_total_duration_ms() {
        let start = SystemTime::now();
        let end = start + std::time::Duration::from_millis(250);

        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Start {
            timestamp: start,
            session_id: "s1".to_string(),
            agent_id: "a1".to_string(),
        });
        log.push(TrajectoryEntry::Finish {
            timestamp: end,
            output: "done".to_string(),
        });

        assert_eq!(log.total_duration_ms(), 250);
    }

    #[test]
    fn test_total_duration_no_start() {
        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Finish {
            timestamp: SystemTime::now(),
            output: "done".to_string(),
        });
        assert_eq!(log.total_duration_ms(), 0);
    }

    #[test]
    fn test_trajectory_entry_serialization() {
        let entry = TrajectoryEntry::Error {
            timestamp: SystemTime::now(),
            message: "oops".to_string(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("oops"));
    }

    // -----------------------------------------------------------------------
    // TrajectoryWriter tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_slugify_basic() {
        assert_eq!(slugify("hello-world"), "hello-world");
        assert_eq!(slugify("hello world!"), "hello-world");
        assert_eq!(slugify("abc_def"), "abcdef");
        assert_eq!(slugify("a"), "a");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn test_slugify_truncates() {
        let long = "a".repeat(50);
        let result = slugify(&long);
        assert_eq!(result.len(), 40);
    }

    #[test]
    fn test_slugify_trim_dashes() {
        assert_eq!(slugify("-hello-"), "hello");
        assert_eq!(slugify("--abc--"), "abc");
    }

    #[test]
    fn test_generate_filename_format() {
        let name = generate_filename("test-session");
        assert!(name.starts_with("trajectory-"));
        assert!(name.ends_with("-test-session.jsonl"));
        // The middle part should be a date-time stamp like 2026-06-09-143022
        let parts: Vec<&str> = name.splitn(3, '-').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[2].ends_with("-test-session.jsonl"));
    }

    #[test]
    fn test_redact_paths() {
        let home = TrajectoryWriter::redact("~/.syscity/config.toml");
        assert_eq!(home, "$HOME/.syscity/config.toml");

        let user_path = TrajectoryWriter::redact("/Users/alice/projects/syscity");
        assert_eq!(user_path, "$HOME/projects/syscity");

        let home_path = TrajectoryWriter::redact("/home/bob/.bashrc");
        assert_eq!(home_path, "$HOME/.bashrc");
    }

    #[test]
    fn test_redact_hex_strings() {
        // 32 hex chars → redacted
        let hex32 = "abcdef0123456789abcdef0123456789";
        assert_eq!(TrajectoryWriter::redact(hex32), "[REDACTED]");

        // 40 hex chars (SHA-1 length) → redacted
        let hex40 = "abcdef0123456789abcdef0123456789abcdef01";
        assert_eq!(TrajectoryWriter::redact(hex40), "[REDACTED]");

        // Short hex (31 chars) → not redacted
        let hex31 = "abcdef0123456789abcdef012345678";
        assert_eq!(TrajectoryWriter::redact(hex31), hex31);
    }

    #[test]
    fn test_redact_combined() {
        let input = "User /Users/alice/.syscity with token abcdef0123456789abcdef0123456789";
        let result = TrajectoryWriter::redact(input);
        assert_eq!(result, "User $HOME/.syscity with token [REDACTED]");
    }

    #[tokio::test]
    async fn test_trajectory_writer_append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let writer = TrajectoryWriter::new().with_dir(dir.path().to_path_buf());

        let entry = TrajectoryEntry::Reasoning {
            timestamp: SystemTime::now(),
            step: "plan".to_string(),
            detail: "thinking...".to_string(),
        };

        writer.append("session-1", &entry).await.unwrap();

        let log = writer.get_trajectory("session-1").await.unwrap();
        assert_eq!(log.entries.len(), 1);
        match &log.entries[0] {
            TrajectoryEntry::Reasoning { step, .. } => assert_eq!(step, "plan"),
            other => panic!("unexpected entry: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_trajectory_writer_append_log() {
        let dir = tempfile::tempdir().unwrap();
        let writer = TrajectoryWriter::new().with_dir(dir.path().to_path_buf());

        let mut log = TrajectoryLog::new();
        log.push(TrajectoryEntry::Start {
            timestamp: SystemTime::now(),
            session_id: "s-1".to_string(),
            agent_id: "agent-1".to_string(),
        });
        log.push(TrajectoryEntry::ToolCall {
            timestamp: SystemTime::now(),
            name: "search".to_string(),
            arguments: serde_json::json!({"q": "rust"}),
        });
        log.push(TrajectoryEntry::Finish {
            timestamp: SystemTime::now(),
            output: "done".to_string(),
        });

        writer.append_log("s-1", &log).await.unwrap();

        let loaded = writer.get_trajectory("s-1").await.unwrap();
        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.tool_calls().len(), 1);
    }

    #[tokio::test]
    async fn test_trajectory_writer_list_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let writer = TrajectoryWriter::new().with_dir(dir.path().to_path_buf());

        // No files yet.
        let sessions = writer.list_sessions().await.unwrap();
        assert!(sessions.is_empty());

        // Write entries for two sessions.
        let entry = TrajectoryEntry::Reasoning {
            timestamp: SystemTime::now(),
            step: "x".to_string(),
            detail: "y".to_string(),
        };
        writer.append("alpha", &entry).await.unwrap();
        writer.append("beta", &entry).await.unwrap();

        let sessions = writer.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions.contains(&"alpha".to_string()));
        assert!(sessions.contains(&"beta".to_string()));
    }

    #[tokio::test]
    async fn test_trajectory_writer_export_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let writer = TrajectoryWriter::new().with_dir(dir.path().to_path_buf());

        let entry = TrajectoryEntry::Reasoning {
            timestamp: SystemTime::now(),
            step: "plan".to_string(),
            detail: "user home is /Users/testuser".to_string(),
        };
        writer.append("export-session", &entry).await.unwrap();

        let export_path = writer.export_bundle("export-session").await.unwrap();

        assert!(export_path.exists());
        let content = tokio::fs::read_to_string(&export_path).await.unwrap();
        // Paths should be redacted in the export.
        assert!(!content.contains("/Users/testuser"));
        assert!(content.contains("$HOME"));
    }
}
