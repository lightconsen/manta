//! Tool-output spill: oversized successful results are written to a file
//! inside the agent's workspace and replaced with a head/tail preview plus
//! a retrieval hint, instead of being truncated inline.
//!
//! The spill directory lives under the workspace root so that the sandboxed
//! `file_read` / `grep` tools can read spilled content back under
//! `workspace_only`. Files accumulate without GC for now (they are small);
//! a retention policy can be added later.
//!
//! Note: the spilled file keeps the raw, unfiltered output. The content
//! filter applies to the model-facing preview only — matching the upstream
//! semantics this design borrows from.

use std::io;
use std::path::{Path, PathBuf};

/// Subdirectory (inside the workspace root) holding spilled outputs.
const SPILL_DIR: &str = ".syscity/spill";

/// The result of spilling one tool output to disk.
#[derive(Debug)]
pub struct SpillOutcome {
    /// Absolute path of the written spill file.
    pub path: PathBuf,
    /// Path relative to the workspace root, for model-facing messages.
    pub rel_path: String,
    /// Total bytes of the original output.
    pub total_bytes: usize,
    /// The replacement text shown to the model (head/tail preview + hint).
    pub replacement: String,
}

/// Truncate `text` to at most `max` bytes, keeping the head and the tail
/// with an omission marker in between. Long-running command errors usually
/// sit at the END of the output, so tail preservation matters.
///
/// No-op when `text` already fits. The marker's byte cost is reserved from
/// the budget, so the result never exceeds `max`.
pub(crate) fn head_tail_truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    // Reserve the marker cost from the budget. The marker embeds the omitted
    // byte count; sizing it with the total length keeps the digit count
    // stable (omitted < total always).
    let marker_size = format!("\n[... {} bytes omitted ...]\n", text.len()).len();
    if max <= marker_size + 2 {
        // Degenerate budget: plain head truncation, no marker.
        let end = floor_char_boundary(text, max);
        return text[..end].to_string();
    }
    let budget = max - marker_size;
    let head_len = floor_char_boundary(text, budget / 2);
    let tail_len = floor_char_boundary_from_end(text, budget - head_len);
    let omitted = text.len() - head_len - tail_len;
    format!(
        "{}\n[... {} bytes omitted ...]\n{}",
        &text[..head_len],
        omitted,
        &text[text.len() - tail_len..]
    )
}

/// Largest byte index <= `idx` that lands on a char boundary.
fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Largest byte count <= `count` such that the last `count` bytes start on
/// a char boundary.
fn floor_char_boundary_from_end(text: &str, count: usize) -> usize {
    let mut start = text.len().saturating_sub(count);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text.len() - start
}

fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Write `output` to the workspace spill dir and build the replacement
/// preview whose total size stays within `budget`.
pub fn spill_output(
    workspace_root: &Path,
    tool_name: &str,
    output: &str,
    budget: usize,
) -> io::Result<SpillOutcome> {
    let dir = workspace_root.join(SPILL_DIR);
    std::fs::create_dir_all(&dir)?;

    let id = uuid::Uuid::new_v4().to_string();
    let file_name = format!("{}-{}.log", &id[..8], sanitize_tool_name(tool_name));
    let path = dir.join(&file_name);

    // Exclusive create with owner-only permissions: never follow a
    // pre-existing/symlinked path.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        f.write_all(output.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, output)?;
    }

    let rel_path = format!("{}/{}", SPILL_DIR, file_name);
    let notice = format!(
        "\n[... output too large — full content ({} bytes) saved to {}. \
         Use file_read (with limit) or grep on that path to inspect further. ...]\n",
        output.len(),
        rel_path
    );
    let preview_budget = budget.saturating_sub(notice.len());
    let head_len = floor_char_boundary(output, preview_budget * 3 / 5);
    let tail_len = floor_char_boundary_from_end(output, preview_budget - head_len);
    let replacement =
        format!("{}{}{}", &output[..head_len], notice, &output[output.len() - tail_len..]);

    Ok(SpillOutcome {
        path,
        rel_path,
        total_bytes: output.len(),
        replacement,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_tail_passthrough_when_fits() {
        assert_eq!(head_tail_truncate("short", 100), "short");
    }

    #[test]
    fn head_tail_keeps_both_ends_within_budget() {
        let text: String = (0..1000)
            .map(|i| if i % 2 == 0 { 'h' } else { 't' })
            .collect();
        let out = head_tail_truncate(&text, 200);
        assert!(out.len() <= 260, "marker overhead bounded, got {}", out.len());
        assert!(out.starts_with('h'), "head preserved");
        assert!(out.ends_with('t'), "tail preserved");
        assert!(out.contains("bytes omitted"));
    }

    #[test]
    fn head_tail_respects_char_boundaries() {
        // 2-byte chars; an odd split would land mid-char without boundary care.
        let text: String = std::iter::repeat('é').take(200).collect();
        let out = head_tail_truncate(&text, 100);
        assert!(out.len() <= 200);
        assert!(out.contains("bytes omitted"));
    }

    #[test]
    fn spill_writes_full_output_and_previews() {
        let dir = std::env::temp_dir().join(format!("syscity_spill_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let output = "x".repeat(10_000);
        let outcome = spill_output(&dir, "shell", &output, 2_000).unwrap();

        // Full content on disk, preview within budget, hint mentions the path.
        let on_disk = std::fs::read_to_string(&outcome.path).unwrap();
        assert_eq!(on_disk.len(), 10_000);
        assert!(outcome.replacement.len() <= 2_000 + 200, "bounded preview");
        assert!(outcome.replacement.contains(&outcome.rel_path));
        assert!(outcome.replacement.contains("file_read"));
        assert!(outcome.rel_path.starts_with(".syscity/spill/"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spill_fails_cleanly_on_unwritable_dir() {
        let outcome = spill_output(Path::new("/nonexistent-root-xyz/nope"), "shell", "data", 100);
        assert!(outcome.is_err());
    }
}
