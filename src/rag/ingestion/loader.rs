//! Source loaders for Knowledge Base ingestion.
//!
//! Loads documents from file, directory, and glob sources. Supports relative
//! path resolution (relative to the agent directory) and absolute paths.

use std::path::Path;

use serde::Deserialize;

use crate::rag::chunk::ChunkStrategy;

/// A source definition loaded from `kb.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct KnowledgeSource {
    /// Unique ID within an agent (auto-generated if not specified).
    #[serde(default)]
    pub id: Option<String>,
    /// Human-readable name for this source (used in reports).
    #[serde(default)]
    pub name: String,
    /// Source type-specific configuration.
    #[serde(flatten)]
    pub source_type: SourceType,
    /// Optional glob pattern for file/dir sources (e.g. `*.md`).
    /// At the source level, applied to all file/dir loads.
    #[serde(default)]
    pub pattern: Option<String>,
    /// Optional collection override (default: `kb-{agent_id}`).
    #[serde(default)]
    pub collection: Option<String>,
    /// Chunk strategy override (default: agent-level or global default).
    #[serde(default)]
    pub chunk_strategy: Option<ChunkStrategy>,
}

/// Supported source types.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum SourceType {
    /// Single file.
    #[serde(rename = "file")]
    File {
        /// Path to the file (relative to agent dir, or absolute).
        path: String,
    },
    /// Directory of files (optionally filtered by glob pattern).
    #[serde(rename = "dir")]
    Dir {
        /// Path to the directory.
        path: String,
    },
    /// Glob pattern relative to agent dir (recursive match).
    #[serde(rename = "glob")]
    Glob {
        /// Glob pattern (e.g. `**/*.md`).
        pattern: String,
    },
}

/// A loaded document ready for chunking and embedding.
#[derive(Debug, Clone)]
pub struct KnowledgeDocument {
    /// Unique document identifier within the collection (filename stem).
    pub doc_id: String,
    /// Source identifier (usually the relative/original path).
    pub source_id: String,
    /// Full text content.
    pub content: String,
    /// SHA-256 checksum of the raw content.
    pub checksum: String,
    /// Last modification time (Unix timestamp in seconds), if available.
    pub mtime: Option<i64>,
    /// Detected MIME type.
    pub mime_type: String,
}

/// Load documents from a knowledge source, resolving relative paths against
/// the given agent directory.
pub fn load_source(
    source: &KnowledgeSource,
    agent_dir: &Path,
) -> Result<Vec<KnowledgeDocument>, String> {
    match &source.source_type {
        SourceType::File { path } => {
            let full_path = resolve_path(path, agent_dir);
            let doc = load_file(&full_path)?;
            Ok(vec![doc])
        }
        SourceType::Dir { path } => {
            let full_path = resolve_path(path, agent_dir);
            load_dir(&full_path, source.pattern.as_deref())
        }
        SourceType::Glob { pattern } => {
            let agent_docs = load_dir(agent_dir, None)?;
            let filtered: Vec<KnowledgeDocument> = agent_docs
                .into_iter()
                .filter(|doc| {
                    let path = std::path::Path::new(&doc.source_id);
                    let relative = path.strip_prefix(agent_dir).unwrap_or(path);
                    glob_match(pattern, &relative.to_string_lossy())
                })
                .collect();
            Ok(filtered)
        }
    }
}

/// Load a single file as a `KnowledgeDocument`.
pub fn load_file(path: &Path) -> Result<KnowledgeDocument, String> {
    let content = std::fs::read(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let checksum = compute_checksum(&content);
    let mime_type = detect_mime(path);
    let mtime = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

    let text = String::from_utf8_lossy(&content).to_string();
    let doc_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("doc_{}", content.len()));

    Ok(KnowledgeDocument {
        doc_id,
        source_id: path.to_string_lossy().to_string(),
        content: text,
        checksum,
        mtime,
        mime_type: mime_type.to_string(),
    })
}

/// Load all matching files from a directory, optionally filtered by glob.
pub fn load_dir(path: &Path, pattern: Option<&str>) -> Result<Vec<KnowledgeDocument>, String> {
    if !path.is_dir() {
        return Err(format!("Not a directory: {}", path.display()));
    }

    let mut docs = Vec::new();
    let entries = std::fs::read_dir(path)
        .map_err(|e| format!("Failed to read dir {}: {}", path.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Dir entry error: {}", e))?;
        let entry_path = entry.path();

        if !entry_path.is_file() {
            continue;
        }

        // Apply glob filter if present
        if let Some(pat) = pattern {
            let filename = entry_path
                .file_name()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default();
            if !glob_match(pat, &filename) {
                continue;
            }
        }

        match load_file(&entry_path) {
            Ok(doc) => docs.push(doc),
            Err(e) => {
                // Log but continue — don't fail the whole batch for one file
                eprintln!("Warning: skipping {}: {}", entry_path.display(), e);
            }
        }
    }

    Ok(docs)
}

/// Compute a fast SHA-256 checksum from first 4 KB + file length.
pub fn compute_checksum(content: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let prefix = content.len().min(4096);
    hasher.update(&content[..prefix]);
    hasher.update(content.len().to_le_bytes());
    format!("{:x}", hasher.finalize())
}

/// Detect MIME type from file extension.
pub fn detect_mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "json" => "application/json",
        "yaml" | "yml" => "application/x-yaml",
        "toml" => "application/toml",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "text/javascript",
        "ts" => "text/typescript",
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        "go" => "text/x-go",
        "java" => "text/x-java",
        "rb" => "text/x-ruby",
        "sh" => "text/x-shellscript",
        "sql" => "text/x-sql",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Simple glob match (supports `*` and `?` wildcards).
fn glob_match(pattern: &str, filename: &str) -> bool {
    let regex_pattern = pattern_to_regex(pattern);
    // Simple regex matching without external crate
    simple_regex_match(&regex_pattern, filename)
}

/// Convert a simple glob pattern to a regex pattern string.
fn pattern_to_regex(pattern: &str) -> String {
    let mut regex = String::with_capacity(pattern.len() + 2);
    regex.push('^');
    for ch in pattern.chars() {
        match ch {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '.' => regex.push_str("\\."),
            '+' => regex.push_str("\\+"),
            '\\' => regex.push_str("\\\\"),
            '|' => regex.push_str("\\|"),
            '(' => regex.push_str("\\("),
            ')' => regex.push_str("\\)"),
            '[' => regex.push_str("\\["),
            ']' => regex.push_str("\\]"),
            '{' => regex.push_str("\\{"),
            '}' => regex.push_str("\\}"),
            '^' => regex.push_str("\\^"),
            '$' => regex.push_str("\\$"),
            '#' => regex.push_str("\\#"),
            ' ' => regex.push_str("\\ "),
            c => regex.push(c),
        }
    }
    regex.push('$');
    regex
}

/// Minimal regex-like matching (supports `.*`, `.`, `^`, `$`, and escaped chars).
fn simple_regex_match(pattern: &str, text: &str) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text.chars().collect();
    simple_match(&chars, 0, &text_chars, 0)
}

fn simple_match(pattern: &[char], pi: usize, text: &[char], ti: usize) -> bool {
    if pi == pattern.len() {
        return ti == text.len();
    }

    match pattern[pi] {
        '^' => {
            // Anchor at start
            if ti == 0 {
                simple_match(pattern, pi + 1, text, ti)
            } else {
                false
            }
        }
        '$' => {
            // Anchor at end — only matches at end of text
            if pi + 1 == pattern.len() {
                ti == text.len()
            } else {
                simple_match(pattern, pi + 1, text, ti)
            }
        }
        '.' if pi + 1 < pattern.len() && pattern[pi + 1] == '*' => {
            // `.*` — match zero or more of any character
            let mut i = ti;
            loop {
                if simple_match(pattern, pi + 2, text, i) {
                    return true;
                }
                if i >= text.len() {
                    return false;
                }
                i += 1;
            }
        }
        '.' => {
            // `.` — match any single character
            if ti < text.len() {
                simple_match(pattern, pi + 1, text, ti + 1)
            } else {
                false
            }
        }
        '\\' => {
            // Escaped character — skip backslash, match next char literally
            if pi + 1 < pattern.len() && ti < text.len() && pattern[pi + 1] == text[ti] {
                simple_match(pattern, pi + 2, text, ti + 1)
            } else {
                false
            }
        }
        c => {
            if ti < text.len() && c == text[ti] {
                simple_match(pattern, pi + 1, text, ti + 1)
            } else {
                false
            }
        }
    }
}

/// Resolve a path string: if absolute, use as-is; otherwise resolve relative
/// to `agent_dir`.
fn resolve_path(path: &str, agent_dir: &Path) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        agent_dir.join(p)
    }
}

/// Load the KB configuration from an agent directory. Returns `None` if the
/// file doesn't exist.
pub fn load_kb_config(agent_dir: &Path) -> Option<Vec<KnowledgeSource>> {
    let config_path = agent_dir.join("kb.toml");
    if !config_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(config_path).ok()?;
    #[derive(Deserialize)]
    struct KbConfig {
        #[serde(default)]
        source: Vec<KnowledgeSource>,
    }
    let cfg: KbConfig = toml::from_str(&content).ok()?;
    Some(cfg.source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_checksum() {
        let content = b"hello world";
        let checksum = compute_checksum(content);
        // SHA-256 of "hello world" prefix + content.len()
        // Just verify it's a 64-char hex string
        assert_eq!(checksum.len(), 64);
        assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_detect_mime() {
        assert_eq!(detect_mime(Path::new("file.md")), "text/markdown");
        assert_eq!(detect_mime(Path::new("file.txt")), "text/plain");
        assert_eq!(detect_mime(Path::new("file.rs")), "text/x-rust");
        assert_eq!(detect_mime(Path::new("file.py")), "text/x-python");
        assert_eq!(detect_mime(Path::new("unknown.xyz")), "application/octet-stream");
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("*.md", "readme.md"));
        assert!(!glob_match("*.md", "readme.txt"));
        assert!(glob_match("doc?.txt", "doc1.txt"));
        assert!(!glob_match("doc?.txt", "doc10.txt"));
        assert!(glob_match("*.*", "main.rs"));
    }

    #[test]
    fn test_resolve_path_absolute() {
        let agent_dir = Path::new("/tmp/agent");
        let abs = resolve_path("/etc/config.toml", agent_dir);
        assert_eq!(abs, Path::new("/etc/config.toml"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let agent_dir = Path::new("/tmp/agent");
        let rel = resolve_path("kb/docs", agent_dir);
        assert_eq!(rel, Path::new("/tmp/agent/kb/docs"));
    }

    #[test]
    fn test_load_file_not_found() {
        let result = load_file(Path::new("/nonexistent/file.md"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_dir_not_found() {
        let result = load_dir(Path::new("/nonexistent"), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_pattern_to_regex() {
        let re = pattern_to_regex("*.rs");
        assert_eq!(re, r"^.*\.rs$");
    }
}
