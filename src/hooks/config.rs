//! Parsing and validation of the Claude-Code-compatible `hooks.json`.
//!
//! Schema (a strict subset of CC's `hooks` setting):
//!
//! ```json
//! {
//!   "version": 1,
//!   "hooks": {
//!     "PreToolUse": [
//!       { "matcher": "Read|Write", "hooks": [ { "type": "command", "command": "./guard.sh" } ] }
//!     ],
//!     "UserPromptSubmit": [
//!       { "hooks": [ { "type": "command", "command": "./gate.sh" } ] }
//!     ]
//!   }
//! }
//! ```
//!
//! Validation is **fail-open**: malformed entries, unknown events, unknown
//! hook types, and version mismatches are `warn!`-logged and skipped rather
//! than aborting the daemon. A missing file yields no configuration; a file
//! that fails to parse yields an empty configuration.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use tracing::warn;

use crate::hooks::matcher::MatchedHook;

/// The four events a shell hook can attach to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// Before a tool executes.
    PreToolUse,
    /// After a tool has finished.
    PostToolUse,
    /// A user message is about to be dispatched to an agent.
    UserPromptSubmit,
    /// An agent turn is stopping.
    Stop,
}

/// Parsed configuration of shell hooks, one vector per event.
#[derive(Debug, Clone, Default)]
pub struct ShellHooksConfig {
    /// Hooks run before matching tools execute (glob-matched).
    pub pre_tool_use: Vec<MatchedHook>,
    /// Hooks run after matching tools finish (glob-matched).
    pub post_tool_use: Vec<MatchedHook>,
    /// Hooks run for every dispatched user message.
    pub user_prompt_submit: Vec<MatchedHook>,
    /// Hooks run fire-and-forget when an agent turn stops.
    pub stop: Vec<MatchedHook>,
}

impl ShellHooksConfig {
    /// Load and parse `hooks.json` at `path`.
    ///
    /// Returns `None` when the file does not exist (no shell hooks
    /// configured). A file that exists but cannot be read or parsed yields
    /// `Some(empty)` after logging a `warn!` — the daemon must never fail to
    /// boot because of a broken hooks file.
    pub fn load(path: &Path) -> Option<ShellHooksConfig> {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                warn!("failed to read hooks file {}: {}", path.display(), e);
                return Some(ShellHooksConfig::default());
            }
        };
        Some(parse_str(&contents))
    }

    /// `true` when no hooks are configured for any event.
    pub fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty()
            && self.post_tool_use.is_empty()
            && self.user_prompt_submit.is_empty()
            && self.stop.is_empty()
    }
}

/// Parse a `hooks.json` document into a [`ShellHooksConfig`].
///
/// Always succeeds; invalid input degrades to an empty configuration.
pub fn parse_str(contents: &str) -> ShellHooksConfig {
    let parsed: Result<HooksFile, _> = serde_json::from_str(contents);
    let file = match parsed {
        Ok(f) => f,
        Err(e) => {
            warn!("hooks.json parse error: {}", e);
            return ShellHooksConfig::default();
        }
    };

    if let Some(version) = file.version {
        if version != 1 {
            warn!(
                "hooks.json version {} is unsupported (expected 1); continuing with best-effort parsing",
                version
            );
        }
    }

    let mut cfg = ShellHooksConfig::default();
    for (key, entries) in file.hooks {
        let event = match key.as_str() {
            "PreToolUse" => HookEvent::PreToolUse,
            "PostToolUse" => HookEvent::PostToolUse,
            "UserPromptSubmit" => HookEvent::UserPromptSubmit,
            "Stop" => HookEvent::Stop,
            other => {
                warn!("hooks.json: unknown hook event '{}' skipped", other);
                continue;
            }
        };
        for entry in entries {
            append_entries(&mut cfg, event, entry);
        }
    }
    cfg
}

/// Flatten one matcher entry's `command` hooks into `cfg` for `event`.
fn append_entries(cfg: &mut ShellHooksConfig, event: HookEvent, entry: MatcherEntry) {
    // A missing matcher matches everything (CC semantics).
    let matcher = entry.matcher.unwrap_or_else(|| "*".to_string());
    let mut flattened = Vec::new();
    for hook in entry.hooks {
        if hook.ty != "command" {
            warn!(
                "hooks.json: hook type '{:?}' is not supported (only 'command'); skipped",
                hook.ty
            );
            continue;
        }
        let Some(command) = hook.command else {
            warn!("hooks.json: 'command' hook without a command; skipped");
            continue;
        };
        flattened.push(MatchedHook {
            matcher: matcher.clone(),
            command,
        });
    }
    match event {
        HookEvent::PreToolUse => cfg.pre_tool_use.extend(flattened),
        HookEvent::PostToolUse => cfg.post_tool_use.extend(flattened),
        HookEvent::UserPromptSubmit => cfg.user_prompt_submit.extend(flattened),
        HookEvent::Stop => cfg.stop.extend(flattened),
    }
}

// ── Raw serde schema ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct HooksFile {
    #[serde(default)]
    version: Option<u64>,
    #[serde(default)]
    hooks: HashMap<String, Vec<MatcherEntry>>,
}

#[derive(Debug, Deserialize)]
struct MatcherEntry {
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    hooks: Vec<HookEntry>,
}

#[derive(Debug, Deserialize)]
struct HookEntry {
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    command: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::matcher::MatchedHook;

    const SAMPLE: &str = r#"{
        "version": 1,
        "hooks": {
            "PreToolUse": [
                { "matcher": "Read|Write", "hooks": [ { "type": "command", "command": "./pre.sh" } ] }
            ],
            "PostToolUse": [
                { "hooks": [ { "type": "command", "command": "./post.sh" } ] }
            ],
            "UserPromptSubmit": [
                { "matcher": "ignored-for-prompt", "hooks": [ { "type": "command", "command": "./gate.sh" } ] }
            ],
            "Stop": [
                { "hooks": [ { "type": "command", "command": "./stop.sh" } ] }
            ]
        }
    }"#;

    #[test]
    fn test_parse_all_four_events() {
        let cfg = parse_str(SAMPLE);
        assert_eq!(cfg.pre_tool_use.len(), 1);
        assert_eq!(cfg.pre_tool_use[0].matcher, "Read|Write");
        assert_eq!(cfg.pre_tool_use[0].command, "./pre.sh");
        assert_eq!(cfg.post_tool_use.len(), 1);
        // Missing matcher defaults to "*".
        assert_eq!(cfg.post_tool_use[0].matcher, "*");
        assert_eq!(cfg.post_tool_use[0].command, "./post.sh");
        // A matcher on UserPromptSubmit is parsed but irrelevant (matching
        // only applies to tool events); it must not break parsing.
        assert_eq!(cfg.user_prompt_submit.len(), 1);
        assert_eq!(cfg.user_prompt_submit[0].matcher, "ignored-for-prompt");
        assert_eq!(cfg.stop.len(), 1);
        assert_eq!(cfg.stop[0].command, "./stop.sh");
    }

    #[test]
    fn test_version_mismatch_warns_but_parses() {
        let cfg = parse_str(
            r#"{"version": 2, "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "x" }] }] }}"#,
        );
        assert_eq!(cfg.stop.len(), 1);
    }

    #[test]
    fn test_unknown_event_skipped() {
        let cfg = parse_str(r#"{"hooks": { "BogusEvent": [{ "hooks": [] }] }}"#);
        assert!(cfg.is_empty());
    }

    #[test]
    fn test_unknown_hook_type_skipped() {
        let cfg = parse_str(
            r#"{"hooks": { "PreToolUse": [{ "hooks": [ { "type": "mcp", "mcpId": "x" } ] }] }}"#,
        );
        assert!(cfg.pre_tool_use.is_empty());
    }

    #[test]
    fn test_command_hook_without_command_skipped() {
        let cfg = parse_str(r#"{"hooks": { "Stop": [{ "hooks": [{ "type": "command" }] }] }}"#);
        assert!(cfg.stop.is_empty());
    }

    #[test]
    fn test_unknown_and_known_mixed() {
        let cfg = parse_str(
            r#"{"hooks": {
                "PreToolUse": [{ "hooks": [ { "type": "command", "command": "ok.sh" } ] }],
                "PluginFoo": [{ "hooks": [ { "type": "command", "command": "x" } ] }]
            }}"#,
        );
        assert_eq!(cfg.pre_tool_use.len(), 1);
        assert_eq!(cfg.pre_tool_use[0].command, "ok.sh");
    }

    #[test]
    fn test_missing_file_returns_none() {
        let cfg = ShellHooksConfig::load(Path::new("/nonexistent/nope/hooks.json"));
        assert!(cfg.is_none());
    }

    #[test]
    fn test_invalid_json_returns_empty_not_panic() {
        let cfg = parse_str("this is not json {");
        assert!(cfg.is_empty());
    }

    #[test]
    fn test_empty_json_is_empty_config() {
        let cfg = parse_str("{}");
        assert!(cfg.is_empty());
    }

    #[test]
    fn test_is_empty_false_when_configured() {
        let cfg = parse_str(SAMPLE);
        assert!(!cfg.is_empty());
        // A configured MatchedHook type-check sanity.
        let _h: MatchedHook = MatchedHook {
            matcher: "*".into(),
            command: "true".into(),
        };
        let _ = cfg.pre_tool_use.first();
    }
}
