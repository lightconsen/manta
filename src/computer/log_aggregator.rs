//! Cross-platform log aggregation and real-time alerting for desktop agents.
//!
//! `LogAggregator` tails system logs (syslog, journald, Windows Event Log,
//! macOS unified log) and arbitrary log files, parsing entries into a
//! structured [`LogEntry`] stream.  [`LogAlertRule`]s can be registered to
//! fire actions when matching log lines arrive.
//!
//! # Usage
//!
//! ```rust,no_run
//! use syscity::computer::log_aggregator::{AlertAction, LogAggregator, LogAlertRule, LogSource};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut aggregator = LogAggregator::new();
//!
//! // Tail a log file
//! aggregator
//!     .add_source(LogSource::file_tail("/var/log/app.log"))
//!     .await?;
//!
//! // Register an alert rule
//! aggregator.add_rule(LogAlertRule {
//!     name: "error-alert".to_string(),
//!     pattern: regex::Regex::new(r"(?i)error|fatal|panic").unwrap(),
//!     min_level: syscity::computer::log_aggregator::LogLevel::Error,
//!     cooldown_secs: 60,
//!     action: AlertAction::NotifyAgent,
//! });
//!
//! // Subscribe to structured log entries
//! let mut rx = aggregator.subscribe();
//! while let Ok(entry) = rx.recv().await {
//!     println!("[{}] {}: {}", entry.source, entry.level, entry.message);
//! }
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Utc};
use regex::Regex;
use tokio::io::AsyncBufReadExt;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::interval;
use tracing::warn;

#[allow(clippy::unwrap_used)]
static RE_ISO_TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[+-]\d{2}:\d{2}|Z)?\s*").unwrap()
});
#[allow(clippy::unwrap_used)]
static RE_SYSLOG_TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2}\s*").unwrap()
});
#[allow(clippy::unwrap_used)]
static RE_BRACKET_LEVEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*\[(DEBUG|INFO|WARN|WARNING|ERROR|FATAL|PANIC|CRIT|TRACE)\]\s*(.*)$").unwrap()
});
#[allow(clippy::unwrap_used)]
static RE_COLON_LEVEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(DEBUG|INFO|WARN|WARNING|ERROR|FATAL|PANIC|CRIT|TRACE)[\s:=\-]+(.*)$").unwrap()
});
#[allow(clippy::unwrap_used)]
static RE_PROCESS_INFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\S+?)\[(\d+)\]\s*:\s*(.*)$").unwrap());

/// Severity level of a log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Verbose debug information.
    Debug,
    /// General informational messages.
    Info,
    /// Warning — non-fatal issues.
    Warning,
    /// Error — operation failed.
    Error,
    /// Fatal / panic — system unusable.
    Fatal,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warning => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
            LogLevel::Fatal => write!(f, "FATAL"),
        }
    }
}

impl LogLevel {
    /// Parse a level string (case-insensitive, common aliases).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "debug" | "dbg" | "trace" => Some(LogLevel::Debug),
            "info" | "information" | "notice" => Some(LogLevel::Info),
            "warn" | "warning" => Some(LogLevel::Warning),
            "error" | "err" | "failure" => Some(LogLevel::Error),
            "fatal" | "panic" | "crit" | "critical" | "emerg" | "alert" => Some(LogLevel::Fatal),
            _ => None,
        }
    }
}

/// A single structured log entry produced by any source.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    /// UTC timestamp of the log event.
    pub timestamp: DateTime<Utc>,
    /// Source identifier (e.g. "syslog", "journald", "app.log").
    pub source: String,
    /// Severity level.
    pub level: LogLevel,
    /// Raw log message text.
    pub message: String,
    /// Process name that emitted the log, if known.
    pub process: Option<String>,
    /// Process ID, if known.
    pub pid: Option<u32>,
}

/// A log source that the aggregator can tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogSource {
    /// Tail a plain text log file.
    FileTail { path: PathBuf },
    /// Tail systemd journal for an optional unit.
    #[cfg(target_os = "linux")]
    Journald { unit: Option<String> },
    /// macOS unified log stream.
    #[cfg(target_os = "macos")]
    MacOsLog {
        predicate: Option<String>,
        level: String,
    },
    /// Windows Event Log channel.
    #[cfg(target_os = "windows")]
    WindowsEvent { channel: String },
}

impl LogSource {
    /// Convenience constructor for file tail.
    pub fn file_tail<P: AsRef<Path>>(path: P) -> Self {
        Self::FileTail {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Return a human-readable source name.
    pub fn name(&self) -> String {
        match self {
            LogSource::FileTail { path } => format!("file:{}", path.display()),
            #[cfg(target_os = "linux")]
            LogSource::Journald { unit: Some(u) } => format!("journald:{u}"),
            #[cfg(target_os = "linux")]
            LogSource::Journald { unit: None } => "journald".to_string(),
            #[cfg(target_os = "macos")]
            LogSource::MacOsLog { .. } => "macos_log".to_string(),
            #[cfg(target_os = "windows")]
            LogSource::WindowsEvent { channel } => format!("winevt:{channel}"),
        }
    }
}

/// Action taken when an alert rule fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertAction {
    /// Send the alert to the active Agent session.
    NotifyAgent,
    /// POST to an external webhook URL.
    SendWebhook(String),
    /// Trigger the rollback manager.
    TriggerRollback,
}

/// A rule that matches log entries and fires an action.
#[derive(Debug, Clone)]
pub struct LogAlertRule {
    /// Human-readable rule name.
    pub name: String,
    /// Regex pattern to match against the log message.
    pub pattern: Regex,
    /// Minimum severity level required to trigger.
    pub min_level: LogLevel,
    /// Cooldown between repeated triggers (seconds).
    pub cooldown_secs: u64,
    /// Action to take on match.
    pub action: AlertAction,
}

/// Internal state tracking per-rule cooldowns.
struct RuleState {
    last_fired: Option<Instant>,
}

/// Aggregates multiple log sources into a single structured stream.
pub struct LogAggregator {
    sources: Vec<(LogSource, JoinHandle<()>)>,
    rules: Arc<Mutex<Vec<(LogAlertRule, RuleState)>>>,
    tx: broadcast::Sender<LogEntry>,
    alert_tx: mpsc::Sender<AlertEvent>,
    alert_rx: Arc<Mutex<mpsc::Receiver<AlertEvent>>>,
}

/// An alert event produced by a rule match.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertEvent {
    pub rule_name: String,
    pub action: AlertAction,
    pub entry: LogEntry,
    pub timestamp: DateTime<Utc>,
}

impl LogAggregator {
    /// Create a new aggregator with default broadcast channel capacity.
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(256);
        let (alert_tx, alert_rx) = mpsc::channel(64);
        Self {
            sources: Vec::new(),
            rules: Arc::new(Mutex::new(Vec::new())),
            tx,
            alert_tx,
            alert_rx: Arc::new(Mutex::new(alert_rx)),
        }
    }

    /// Subscribe to the broadcast stream of [`LogEntry`]s.
    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.tx.subscribe()
    }

    /// Subscribe to alert events.
    pub fn alert_receiver(&self) -> Arc<Mutex<mpsc::Receiver<AlertEvent>>> {
        self.alert_rx.clone()
    }

    /// Add a structured log source and start tailing it.
    pub async fn add_source(&mut self, source: LogSource) -> crate::computer::Result<()> {
        let name = source.name();
        let tx = self.tx.clone();

        let handle = match &source {
            LogSource::FileTail { path } => {
                let path = path.clone();
                tokio::spawn(async move {
                    if let Err(e) = tail_file(&path, &name, tx).await {
                        warn!("File tail {} failed: {}", path.display(), e);
                    }
                })
            }
            #[cfg(target_os = "linux")]
            LogSource::Journald { unit } => {
                let unit = unit.clone();
                tokio::spawn(async move {
                    if let Err(e) = tail_journald(unit.as_deref(), &name, tx).await {
                        warn!("Journald tail failed: {}", e);
                    }
                })
            }
            #[cfg(target_os = "macos")]
            LogSource::MacOsLog { predicate, level } => {
                let predicate = predicate.clone();
                let level = level.clone();
                tokio::spawn(async move {
                    if let Err(e) = tail_macos_log(predicate.as_deref(), &level, &name, tx).await {
                        warn!("macOS log tail failed: {}", e);
                    }
                })
            }
            #[cfg(target_os = "windows")]
            LogSource::WindowsEvent { channel } => {
                let channel = channel.clone();
                tokio::spawn(async move {
                    if let Err(e) = tail_windows_event(&channel, &name, tx).await {
                        warn!("Windows event tail failed: {}", e);
                    }
                })
            }
        };

        self.sources.push((source, handle));
        Ok(())
    }

    /// Register an alert rule.
    pub async fn add_rule(&self, rule: LogAlertRule) {
        let mut rules = self.rules.lock().await;
        rules.push((rule, RuleState { last_fired: None }));
    }

    /// Start the alert evaluator task.  Should be called once after all
    /// rules and sources are set up.
    pub fn start_alert_evaluator(&self) -> JoinHandle<()> {
        let mut rx = self.subscribe();
        let rules = self.rules.clone();
        let alert_tx = self.alert_tx.clone();

        tokio::spawn(async move {
            while let Ok(entry) = rx.recv().await {
                let mut guard = rules.lock().await;
                for (rule, state) in guard.iter_mut() {
                    if entry.level < rule.min_level {
                        continue;
                    }
                    if !rule.pattern.is_match(&entry.message) {
                        continue;
                    }
                    let now = Instant::now();
                    if let Some(last) = state.last_fired {
                        if now.duration_since(last) < Duration::from_secs(rule.cooldown_secs) {
                            continue;
                        }
                    }
                    state.last_fired = Some(now);
                    let event = AlertEvent {
                        rule_name: rule.name.clone(),
                        action: rule.action.clone(),
                        entry: entry.clone(),
                        timestamp: Utc::now(),
                    };
                    if alert_tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        })
    }

    /// Stop all source tail tasks.
    pub async fn shutdown(&mut self) {
        for (_, handle) in self.sources.drain(..) {
            handle.abort();
        }
    }
}

impl Default for LogAggregator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// File tail implementation
// ---------------------------------------------------------------------------

async fn tail_file(
    path: &Path,
    source_name: &str,
    tx: broadcast::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::fs::File;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let file = File::open(path).await?;
    // Seek to end — we only want new lines.
    let metadata = file.metadata().await?;
    let mut reader = BufReader::new(file);
    let len = metadata.len();
    if len > 0 {
        tokio::io::AsyncSeekExt::seek(&mut reader, std::io::SeekFrom::Start(len)).await?;
    }

    let mut line = String::new();
    let mut interval = interval(Duration::from_millis(500));

    loop {
        interval.tick().await;
        loop {
            line.clear();
            match reader.read_line(&mut line).await? {
                0 => break,
                _ => {
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Some(entry) = parse_generic_log_line(trimmed, source_name) {
                        let _ = tx.send(entry);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Linux journald implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
async fn tail_journald(
    unit: Option<&str>,
    source_name: &str,
    tx: broadcast::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut cmd = tokio::process::Command::new("journalctl");
    cmd.arg("-f")
        .arg("-o")
        .arg("short-iso")
        .arg("--no-pager")
        .arg("--quiet");
    if let Some(u) = unit {
        cmd.arg("-u").arg(u);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut reader = tokio::io::BufReader::new(stdout).lines();

    while let Some(line) = reader.next_line().await? {
        if let Some(entry) = parse_journald_line(&line, source_name) {
            let _ = tx.send(entry);
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
async fn tail_journald(
    _unit: Option<&str>,
    _source_name: &str,
    _tx: broadcast::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err("journald only available on Linux".into())
}

// ---------------------------------------------------------------------------
// macOS unified log implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
async fn tail_macos_log(
    predicate: Option<&str>,
    level: &str,
    source_name: &str,
    tx: broadcast::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut cmd = tokio::process::Command::new("log");
    cmd.arg("stream")
        .arg("--level")
        .arg(level)
        .arg("--style")
        .arg("json");
    if let Some(p) = predicate {
        cmd.arg("--predicate").arg(p);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut reader = tokio::io::BufReader::new(stdout).lines();

    while let Some(line) = reader.next_line().await? {
        if let Some(entry) = parse_macos_log_line(&line, source_name) {
            let _ = tx.send(entry);
        }
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
async fn tail_macos_log(
    _predicate: Option<&str>,
    _level: &str,
    _source_name: &str,
    _tx: broadcast::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err("macOS unified log only available on macOS".into())
}

// ---------------------------------------------------------------------------
// Windows Event Log implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
async fn tail_windows_event(
    channel: &str,
    source_name: &str,
    tx: broadcast::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Use wevtutil to query recent events and then poll.
    let mut cmd = tokio::process::Command::new("wevtutil");
    cmd.arg("qe").arg(channel).arg("/f:text").arg("/c:1");
    cmd.stdout(std::process::Stdio::piped());

    let mut last_check = Instant::now();
    let mut interval = interval(Duration::from_secs(5));

    loop {
        interval.tick().await;
        cmd.stdout(std::process::Stdio::piped());
        let output = cmd.output().await?;
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if let Some(entry) = parse_windows_event_line(line, source_name) {
                // Only emit events newer than last check
                if entry.timestamp > DateTime::from(last_check) {
                    let _ = tx.send(entry);
                }
            }
        }
        last_check = Instant::now();
    }
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
async fn tail_windows_event(
    _channel: &str,
    _source_name: &str,
    _tx: broadcast::Sender<LogEntry>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    Err("Windows Event Log only available on Windows".into())
}

// ---------------------------------------------------------------------------
// Log line parsers
// ---------------------------------------------------------------------------

/// Best-effort generic parser for common log formats.
///
/// Recognises:
/// - RFC 3339 / ISO 8601 timestamps
/// - Standard syslog-style severity prefixes (ERROR, WARN, INFO, DEBUG, FATAL)
/// - Simple `level: message` or `[level] message` patterns
fn parse_generic_log_line(line: &str, source_name: &str) -> Option<LogEntry> {
    // Try to extract timestamp.
    let ts = extract_timestamp(line).unwrap_or_else(Utc::now);

    // Strip the timestamp prefix so level extraction works on the rest.
    let after_ts = strip_timestamp_prefix(line);

    // Try to extract level.
    let (level, remaining) = extract_level_and_remainder(after_ts);

    // Try to extract process name / PID from common prefixes.
    let (process, pid, message) = extract_process_info(remaining);

    Some(LogEntry {
        timestamp: ts,
        source: source_name.to_string(),
        level,
        message: message.to_string(),
        process,
        pid,
    })
}

#[cfg(target_os = "linux")]
fn parse_journald_line(line: &str, source_name: &str) -> Option<LogEntry> {
    // journalctl -f -o short-iso format:
    // 2024-01-15T10:30:00+0800 hostname process[pid]: message
    let re = Regex::new(
        r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}[+-]\d{4})\s+\S+\s+(\S+?)(?:\[(\d+)\])?\s*:\s*(.*)$",
    )
    .ok()?;
    let caps = re.captures(line)?;

    let ts_str = caps.get(1)?.as_str();
    let ts = DateTime::parse_from_str(ts_str, "%Y-%m-%dT%H:%M:%S%z")
        .ok()
        .map(|dt| dt.with_timezone(&Utc))?;
    let process = caps.get(2).map(|m| m.as_str().to_string());
    let pid = caps.get(3).and_then(|m| m.as_str().parse().ok());
    let message = caps.get(4)?.as_str().to_string();

    let (level, _) = extract_level_and_remainder(&message);

    Some(LogEntry {
        timestamp: ts,
        source: source_name.to_string(),
        level,
        message,
        process,
        pid,
    })
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn parse_journald_line(_line: &str, _source_name: &str) -> Option<LogEntry> {
    None
}

#[cfg(target_os = "macos")]
fn parse_macos_log_line(line: &str, source_name: &str) -> Option<LogEntry> {
    // log stream --style json produces one JSON object per line.
    // Fallback: treat as plain text with timestamp heuristic.
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
        let ts = json
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let message = json
            .get("eventMessage")
            .or_else(|| json.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or(line)
            .to_string();
        let process = json
            .get("process")
            .and_then(|v| v.as_str())
            .map(String::from);
        let pid = json
            .get("processID")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        let level_str = json
            .get("messageType")
            .or_else(|| json.get("level"))
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let level = LogLevel::parse(level_str).unwrap_or(LogLevel::Info);

        return Some(LogEntry {
            timestamp: ts,
            source: source_name.to_string(),
            level,
            message,
            process,
            pid,
        });
    }

    // Plain-text fallback.
    parse_generic_log_line(line, source_name)
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
fn parse_macos_log_line(_line: &str, _source_name: &str) -> Option<LogEntry> {
    None
}

#[cfg(target_os = "windows")]
fn parse_windows_event_line(line: &str, source_name: &str) -> Option<LogEntry> {
    // wevtutil /f:text produces multi-line blocks; we process per-line here.
    // Look for Level/EventID lines and extract message body.
    parse_generic_log_line(line, source_name)
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn parse_windows_event_line(_line: &str, _source_name: &str) -> Option<LogEntry> {
    None
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Strip a recognized timestamp prefix from the start of a log line.
fn strip_timestamp_prefix(line: &str) -> &str {
    // ISO 8601 / RFC 3339.
    let iso_re = RE_ISO_TIMESTAMP.clone();
    if let Some(m) = iso_re.find(line) {
        return line[m.end()..].trim_start();
    }
    // Syslog style: "Jan 15 10:30:00".
    let syslog_re = RE_SYSLOG_TIMESTAMP.clone();
    if let Some(m) = syslog_re.find(line) {
        return line[m.end()..].trim_start();
    }
    line
}

fn extract_timestamp(line: &str) -> Option<DateTime<Utc>> {
    // Try ISO 8601 / RFC 3339 at the start of the line.
    let iso_re =
        Regex::new(r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:[+-]\d{2}:\d{2}|Z)?)")
            .ok()?;
    if let Some(caps) = iso_re.captures(line) {
        let s = caps.get(1)?.as_str();
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        // Try without timezone offset (naive, assume UTC).
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
            return Some(DateTime::from_naive_utc_and_offset(dt, Utc));
        }
    }

    // Try syslog-style: "Jan 15 10:30:00".
    let syslog_re = Regex::new(r"^([A-Z][a-z]{2}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})").ok()?;
    if let Some(caps) = syslog_re.captures(line) {
        let s = caps.get(1)?.as_str();
        let fmt = "%b %d %H:%M:%S";
        let now = Utc::now();
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(
            &format!("{} {}", now.year(), s),
            &format!("%Y {}", fmt),
        ) {
            return Some(DateTime::from_naive_utc_and_offset(dt, Utc));
        }
    }

    None
}

/// Extract severity level from the line and return the remainder text.
fn extract_level_and_remainder(line: &str) -> (LogLevel, &str) {
    // Check for bracketed level: [ERROR], [WARN], etc.
    let bracket_re = RE_BRACKET_LEVEL.clone();
    if let Some(caps) = bracket_re.captures(line) {
        let level_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let rest = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if let Some(level) = LogLevel::parse(level_str) {
            return (level, rest);
        }
    }

    // Check for colon-separated level: ERROR: message, WARN - message.
    let colon_re = RE_COLON_LEVEL.clone();
    if let Some(caps) = colon_re.captures(line) {
        let level_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let rest = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if let Some(level) = LogLevel::parse(level_str) {
            return (level, rest);
        }
    }

    // Default to Info if no explicit level found.
    (LogLevel::Info, line)
}

/// Try to extract process name and PID from prefixes like `process[123]:`.
fn extract_process_info(line: &str) -> (Option<String>, Option<u32>, &str) {
    let re = RE_PROCESS_INFO.clone();
    if let Some(caps) = re.captures(line) {
        let proc = caps.get(1).map(|m| m.as_str().to_string());
        let pid = caps.get(2).and_then(|m| m.as_str().parse().ok());
        let msg = caps.get(3).map(|m| m.as_str()).unwrap_or(line);
        return (proc, pid, msg);
    }
    (None, None, line)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_parse() {
        assert_eq!(LogLevel::parse("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("warn"), Some(LogLevel::Warning));
        assert_eq!(LogLevel::parse("panic"), Some(LogLevel::Fatal));
        assert_eq!(LogLevel::parse("unknown"), None);
    }

    #[test]
    fn test_extract_timestamp_iso() {
        let line = "2024-01-15T10:30:00Z Hello world";
        let ts = extract_timestamp(line).unwrap();
        assert_eq!(ts.year(), 2024);
        assert_eq!(ts.month(), 1);
        assert_eq!(ts.day(), 15);
    }

    #[test]
    fn test_extract_timestamp_syslog() {
        let line = "Jan 15 10:30:00 sshd[1234]: Connection accepted";
        let ts = extract_timestamp(line).unwrap();
        assert_eq!(ts.month(), 1);
        assert_eq!(ts.day(), 15);
    }

    #[test]
    fn test_extract_level_bracket() {
        let line = "[ERROR] database connection failed";
        let (level, rest) = extract_level_and_remainder(line);
        assert_eq!(level, LogLevel::Error);
        assert_eq!(rest, "database connection failed");
    }

    #[test]
    fn test_extract_level_colon() {
        let line = "WARN: low disk space";
        let (level, rest) = extract_level_and_remainder(line);
        assert_eq!(level, LogLevel::Warning);
        assert_eq!(rest, "low disk space");
    }

    #[test]
    fn test_extract_process_info() {
        let line = "sshd[1234]: Connection accepted from 192.168.1.1";
        let (proc, pid, msg) = extract_process_info(line);
        assert_eq!(proc, Some("sshd".to_string()));
        assert_eq!(pid, Some(1234));
        assert_eq!(msg, "Connection accepted from 192.168.1.1");
    }

    #[test]
    fn test_parse_generic_log_line() {
        let line = "2024-01-15T10:30:00Z [ERROR] myapp[42]: something went wrong";
        let entry = parse_generic_log_line(line, "test").unwrap();
        assert_eq!(entry.source, "test");
        assert_eq!(entry.level, LogLevel::Error);
        assert_eq!(entry.process, Some("myapp".to_string()));
        assert_eq!(entry.pid, Some(42));
        assert!(entry.message.contains("something went wrong"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_journald_line() {
        let line = "2024-01-15T10:30:00+0800 myhost sshd[1234]: Connection accepted";
        let entry = parse_journald_line(line, "journald").unwrap();
        assert_eq!(entry.source, "journald");
        assert_eq!(entry.process, Some("sshd".to_string()));
        assert_eq!(entry.pid, Some(1234));
        assert_eq!(entry.message, "Connection accepted");
    }
}
