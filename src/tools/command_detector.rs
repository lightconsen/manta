//! Three-layer command detector
//!
//! Detects commands inside an incoming message before it is routed to the
//! agent.
//!
//! Layers (in priority order):
//!
//! 1. **Control commands** — system-level commands such as `/start`, `/help`,
//!    `/pair`.
//! 2. **Command messages** — messages starting with `/` (or another configured
//!    prefix) followed by a command name and arguments.
//! 3. **Inline tokens** — commands embedded inside conversational text, e.g.
//!    "please run `/skill list`" or "@bot do something".

use crate::channels::command_gate::parse_command;
use crate::tools::command_gate::{ControlCommand, RequestClass};

/// Which detection layer produced a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionLayer {
    /// Exact control command at the start of the message.
    Control,
    /// `/`-prefixed command message.
    CommandMessage,
    /// Command embedded in natural language.
    InlineToken,
}

/// Result of running the command detector on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandDetectionResult {
    /// Which layer matched.
    pub layer: DetectionLayer,
    /// Canonical command name.
    pub command: String,
    /// Parsed arguments, if any.
    pub args: Vec<String>,
    /// The raw substring that was matched.
    pub raw_match: String,
}

impl CommandDetectionResult {
    /// Return true if this is a control command result.
    pub fn is_control(&self) -> bool {
        matches!(self.layer, DetectionLayer::Control)
    }

    /// Return true if this is a command message result.
    pub fn is_command_message(&self) -> bool {
        matches!(self.layer, DetectionLayer::CommandMessage)
    }

    /// Return true if this is an inline token result.
    pub fn is_inline_token(&self) -> bool {
        matches!(self.layer, DetectionLayer::InlineToken)
    }
}

/// Configuration for the detector.
#[derive(Debug, Clone)]
pub struct CommandDetectorConfig {
    /// Prefixes that introduce command messages.
    pub command_prefixes: Vec<String>,
    /// Mentions that introduce inline commands.
    pub mention_prefixes: Vec<String>,
    /// If true, inline token detection is enabled.
    pub enable_inline_detection: bool,
}

impl Default for CommandDetectorConfig {
    fn default() -> Self {
        Self {
            command_prefixes: vec!["/".to_string(), "!".to_string()],
            mention_prefixes: vec!["@bot ".to_string(), "@syscity ".to_string()],
            enable_inline_detection: true,
        }
    }
}

/// Detect commands in a user message.
#[derive(Debug, Clone, Default)]
pub struct CommandDetector {
    config: CommandDetectorConfig,
}

impl CommandDetector {
    /// Create a detector with the default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a detector with a custom configuration.
    pub fn with_config(config: CommandDetectorConfig) -> Self {
        Self { config }
    }

    /// Run all three detection layers and return the highest-priority match.
    pub fn detect(&self, content: &str) -> Option<CommandDetectionResult> {
        // Layer 1: control commands
        if let Some(result) = self.detect_control(content) {
            return Some(result);
        }

        // Layer 2: command messages
        if let Some(result) = self.detect_command_message(content) {
            return Some(result);
        }

        // Layer 3: inline tokens
        if self.config.enable_inline_detection {
            if let Some(result) = self.detect_inline_tokens(content) {
                return Some(result);
            }
        }

        None
    }

    /// Layer 1 — detect system control commands.
    fn detect_control(&self, content: &str) -> Option<CommandDetectionResult> {
        let ctrl = ControlCommand::detect(content)?;
        let trimmed = content.trim();
        let raw = trimmed.split_whitespace().next()?.to_string();
        let args: Vec<String> = trimmed
            .strip_prefix(&raw)
            .unwrap_or("")
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();

        Some(CommandDetectionResult {
            layer: DetectionLayer::Control,
            command: ctrl.as_str().to_string(),
            args,
            raw_match: raw,
        })
    }

    /// Layer 2 — detect `/`-prefixed command messages.
    fn detect_command_message(&self, content: &str) -> Option<CommandDetectionResult> {
        let prefixes: Vec<&str> = self
            .config
            .command_prefixes
            .iter()
            .map(|s| s.as_str())
            .collect();
        let (command, args) = parse_command(content, &prefixes)?;
        let raw = content.split_whitespace().next()?.to_string();

        Some(CommandDetectionResult {
            layer: DetectionLayer::CommandMessage,
            command,
            args,
            raw_match: raw,
        })
    }

    /// Layer 3 — detect commands embedded in chat text.
    fn detect_inline_tokens(&self, content: &str) -> Option<CommandDetectionResult> {
        // Try mention-based triggers first.
        for mention in &self.config.mention_prefixes {
            if let Some(pos) = content.to_lowercase().find(&mention.to_lowercase()) {
                let after = &content[pos + mention.len()..];
                let parts: Vec<&str> = after.split_whitespace().collect();
                if !parts.is_empty() {
                    let command = parts[0].trim_start_matches('/').to_lowercase();
                    let args = parts[1..].iter().map(|s| s.to_string()).collect();
                    let raw = parts.join(" ");
                    return Some(CommandDetectionResult {
                        layer: DetectionLayer::InlineToken,
                        command,
                        args,
                        raw_match: raw,
                    });
                }
            }
        }

        // Then look for backtick-quoted `/command` tokens.
        if let Some(start) = content.find("`/") {
            let rest = &content[start + 2..];
            if let Some(end) = rest.find('`') {
                let token = &rest[..end];
                let parts: Vec<&str> = token.split_whitespace().collect();
                if !parts.is_empty() {
                    let command = parts[0].to_lowercase();
                    let args = parts[1..].iter().map(|s| s.to_string()).collect();
                    let raw = token.to_string();
                    return Some(CommandDetectionResult {
                        layer: DetectionLayer::InlineToken,
                        command,
                        args,
                        raw_match: raw,
                    });
                }
            }
        }

        None
    }
}

/// Convenience function for one-shot detection with default config.
pub fn detect_command(content: &str) -> Option<CommandDetectionResult> {
    CommandDetector::new().detect(content)
}

/// Convert a detection result back to a `RequestClass` for legacy gate checks.
pub fn request_class_from_detection(result: &CommandDetectionResult) -> RequestClass {
    match result.layer {
        DetectionLayer::Control => RequestClass::ControlCommand(
            ControlCommand::detect(&result.raw_match).unwrap_or(ControlCommand::Help),
        ),
        DetectionLayer::CommandMessage | DetectionLayer::InlineToken => {
            // Preserve admin-command distinction for command messages.
            let raw = format!("/{}", result.command);
            RequestClass::classify(&raw)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_control_start() {
        let result = detect_command("/start hello").unwrap();
        assert!(result.is_control());
        assert_eq!(result.command, "start");
        assert_eq!(result.args, vec!["hello"]);
    }

    #[test]
    fn test_detect_control_help() {
        let result = detect_command("/help").unwrap();
        assert!(result.is_control());
        assert_eq!(result.command, "help");
        assert!(result.args.is_empty());
    }

    #[test]
    fn test_detect_command_message() {
        let result = detect_command("/skill list").unwrap();
        assert!(result.is_command_message());
        assert_eq!(result.command, "skill");
        assert_eq!(result.args, vec!["list"]);
    }

    #[test]
    fn test_detect_inline_backtick() {
        let result = detect_command("please run `/skill list` for me").unwrap();
        assert!(result.is_inline_token());
        assert_eq!(result.command, "skill");
        assert_eq!(result.args, vec!["list"]);
    }

    #[test]
    fn test_detect_inline_mention() {
        let result = detect_command("@bot skill list").unwrap();
        assert!(result.is_inline_token());
        assert_eq!(result.command, "skill");
        assert_eq!(result.args, vec!["list"]);
    }

    #[test]
    fn test_detect_no_command() {
        assert!(detect_command("hello there").is_none());
    }

    #[test]
    fn test_control_takes_priority() {
        // /help is a control command; it should not be treated as a generic
        // command message named "help".
        let result = detect_command("/help commands").unwrap();
        assert!(result.is_control());
        assert_eq!(result.command, "help");
    }

    #[test]
    fn test_request_class_from_detection() {
        let result = detect_command("/admin providers").unwrap();
        let class = request_class_from_detection(&result);
        assert_eq!(class, RequestClass::AdminCommand);
    }
}
