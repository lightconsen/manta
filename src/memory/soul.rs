//! Structured SOUL.md configuration parser
//!
//! Implements OpenClaw-style config-as-code for agent personality.
//! SOUL.md files can contain YAML frontmatter with structured fields
//! (name, persona, voice, behavior, preferences) followed by free-form
//! markdown body.
//!
//! # Example SOUL.md
//!
//! ```markdown
//! ---
//! name: Manta
//! persona: Helpful AI assistant with a curious edge
//! voice: concise, direct, no filler
//! emoji: "🦑"
//! behavior:
//!   proactive: true
//!   ask_before_destructive: true
//!   group_chat_mode: smart
//! preferences:
//!   language: en-US
//!   code_style: rust
//!   format: markdown
//! ---
//!
//! # Core Truths
//!
//! Be genuinely helpful, not performatively helpful...
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Structured agent behavior configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BehaviorConfig {
    /// Whether the agent should proactively suggest actions.
    #[serde(default)]
    pub proactive: Option<bool>,
    /// Whether to ask before destructive operations.
    #[serde(default)]
    pub ask_before_destructive: Option<bool>,
    /// Group chat participation mode.
    #[serde(default)]
    pub group_chat_mode: Option<String>,
    /// Additional free-form behavior flags.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

/// User preference configuration embedded in SOUL.md.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferenceConfig {
    /// Preferred language code (e.g. "en-US", "zh-CN").
    #[serde(default)]
    pub language: Option<String>,
    /// Preferred code style conventions.
    #[serde(default)]
    pub code_style: Option<String>,
    /// Preferred response format.
    #[serde(default)]
    pub format: Option<String>,
    /// Additional free-form preferences.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

/// Parsed structured configuration from a SOUL.md frontmatter block.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoulConfig {
    /// Agent name / call sign.
    #[serde(default)]
    pub name: Option<String>,
    /// Short persona description.
    #[serde(default)]
    pub persona: Option<String>,
    /// Voice / tone description.
    #[serde(default)]
    pub voice: Option<String>,
    /// Signature emoji.
    #[serde(default)]
    pub emoji: Option<String>,
    /// Structured behavior flags.
    #[serde(default)]
    pub behavior: BehaviorConfig,
    /// Structured preferences.
    #[serde(default)]
    pub preferences: PreferenceConfig,
    /// Extra top-level keys for forward compatibility.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

impl SoulConfig {
    /// Generate a structured system-prompt fragment from the config.
    ///
    /// Returns an empty string if no structured fields are set.
    pub fn to_prompt_fragment(&self) -> String {
        let mut parts = Vec::new();

        if let Some(name) = &self.name {
            parts.push(format!("**Name**: {}", name));
        }
        if let Some(persona) = &self.persona {
            parts.push(format!("**Persona**: {}", persona));
        }
        if let Some(voice) = &self.voice {
            parts.push(format!("**Voice**: {}", voice));
        }
        if let Some(emoji) = &self.emoji {
            parts.push(format!("**Emoji**: {}", emoji));
        }

        let mut behavior_parts = Vec::new();
        if let Some(v) = self.behavior.proactive {
            behavior_parts.push(format!("- Proactive: {}", v));
        }
        if let Some(v) = self.behavior.ask_before_destructive {
            behavior_parts.push(format!("- Ask before destructive actions: {}", v));
        }
        if let Some(v) = &self.behavior.group_chat_mode {
            behavior_parts.push(format!("- Group chat mode: {}", v));
        }
        for (k, v) in &self.behavior.extra {
            behavior_parts.push(format!("- {}: {}", k, yaml_to_string(v)));
        }
        if !behavior_parts.is_empty() {
            parts.push(format!("**Behavior**:\n{}", behavior_parts.join("\n")));
        }

        let mut pref_parts = Vec::new();
        if let Some(v) = &self.preferences.language {
            pref_parts.push(format!("- Language: {}", v));
        }
        if let Some(v) = &self.preferences.code_style {
            pref_parts.push(format!("- Code style: {}", v));
        }
        if let Some(v) = &self.preferences.format {
            pref_parts.push(format!("- Format: {}", v));
        }
        for (k, v) in &self.preferences.extra {
            pref_parts.push(format!("- {}: {}", k, yaml_to_string(v)));
        }
        if !pref_parts.is_empty() {
            parts.push(format!("**Preferences**:\n{}", pref_parts.join("\n")));
        }

        if parts.is_empty() {
            String::new()
        } else {
            format!("\n### Agent Profile\n\n{}\n", parts.join("\n\n"))
        }
    }
}

fn yaml_to_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        _ => serde_yaml::to_string(v)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

/// A parsed SOUL.md file with optional structured frontmatter and markdown body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoulFile {
    /// Structured configuration from YAML frontmatter (if present).
    pub config: SoulConfig,
    /// Free-form markdown body (everything after the frontmatter).
    pub body: String,
    /// Whether the file contained a valid frontmatter block.
    pub has_frontmatter: bool,
}

impl SoulFile {
    /// Parse raw SOUL.md content into structured config + body.
    pub fn parse(content: &str) -> crate::Result<Self> {
        let trimmed = content.trim_start();

        // Check for YAML frontmatter delimiter
        if !trimmed.starts_with("---") {
            return Ok(Self {
                config: SoulConfig::default(),
                body: content.to_string(),
                has_frontmatter: false,
            });
        }

        // Find the closing ---
        let after_open = &trimmed[3..]; // skip "---"
        let Some(close_idx) = after_open.find("\n---") else {
            // No closing delimiter — treat as body without frontmatter
            return Ok(Self {
                config: SoulConfig::default(),
                body: content.to_string(),
                has_frontmatter: false,
            });
        };

        let yaml_text = after_open[..close_idx].trim();
        let body_start = 3 + close_idx + 4; // "---" + close_idx + "\n---"
        let body = trimmed[body_start.min(trimmed.len())..]
            .trim_start()
            .to_string();

        let config: SoulConfig = serde_yaml::from_str(yaml_text).map_err(|e| {
            crate::error::MantaError::Validation(format!(
                "Failed to parse SOUL.md frontmatter: {}",
                e
            ))
        })?;

        Ok(Self {
            config,
            body,
            has_frontmatter: true,
        })
    }

    /// Merge structured config prompt fragment + body into full prompt text.
    pub fn to_full_prompt(&self) -> String {
        let fragment = self.config.to_prompt_fragment();
        if self.body.is_empty() {
            return fragment;
        }
        if fragment.is_empty() {
            return self.body.clone();
        }
        format!("{}\n{}", fragment, self.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soul_config_prompt_fragment() {
        let config = SoulConfig {
            name: Some("Manta".to_string()),
            persona: Some("Helpful squid".to_string()),
            voice: Some("concise".to_string()),
            emoji: Some("🦑".to_string()),
            behavior: BehaviorConfig {
                proactive: Some(true),
                ask_before_destructive: Some(true),
                group_chat_mode: Some("smart".to_string()),
                extra: HashMap::new(),
            },
            preferences: PreferenceConfig {
                language: Some("zh-CN".to_string()),
                code_style: Some("rust".to_string()),
                format: Some("markdown".to_string()),
                extra: HashMap::new(),
            },
            extra: HashMap::new(),
        };

        let fragment = config.to_prompt_fragment();
        assert!(fragment.contains("**Name**: Manta"));
        assert!(fragment.contains("**Persona**: Helpful squid"));
        assert!(fragment.contains("**Voice**: concise"));
        assert!(fragment.contains("**Emoji**: 🦑"));
        assert!(fragment.contains("Proactive: true"));
        assert!(fragment.contains("Group chat mode: smart"));
        assert!(fragment.contains("Language: zh-CN"));
        assert!(fragment.contains("Code style: rust"));
    }

    #[test]
    fn test_soul_file_parse_with_frontmatter() {
        let content = r#"---
name: Manta
persona: Curious AI
voice: snarky
emoji: "🦑"
behavior:
  proactive: true
---

# Core Truths

Be helpful.
"#;

        let soul = SoulFile::parse(content).unwrap();
        assert!(soul.has_frontmatter);
        assert_eq!(soul.config.name, Some("Manta".to_string()));
        assert_eq!(soul.config.persona, Some("Curious AI".to_string()));
        assert_eq!(soul.config.voice, Some("snarky".to_string()));
        assert_eq!(soul.config.emoji, Some("🦑".to_string()));
        assert_eq!(soul.config.behavior.proactive, Some(true));
        assert!(soul.body.contains("# Core Truths"));
        assert!(soul.body.contains("Be helpful."));
    }

    #[test]
    fn test_soul_file_parse_without_frontmatter() {
        let content = "# Just markdown\n\nNo frontmatter here.";
        let soul = SoulFile::parse(content).unwrap();
        assert!(!soul.has_frontmatter);
        assert_eq!(soul.config.name, None);
        assert_eq!(soul.body, content);
    }

    #[test]
    fn test_soul_file_parse_empty() {
        let soul = SoulFile::parse("").unwrap();
        assert!(!soul.has_frontmatter);
        assert!(soul.body.is_empty());
    }

    #[test]
    fn test_soul_file_to_full_prompt() {
        let soul = SoulFile {
            config: SoulConfig {
                name: Some("Test".to_string()),
                ..Default::default()
            },
            body: "# Body\n\nText.".to_string(),
            has_frontmatter: true,
        };
        let prompt = soul.to_full_prompt();
        assert!(prompt.contains("**Name**: Test"));
        assert!(prompt.contains("# Body"));
    }

    #[test]
    fn test_soul_file_invalid_frontmatter_falls_back() {
        // Opening --- but no closing ---
        let content = "---\nname: Manta\n# No closing delimiter";
        let soul = SoulFile::parse(content).unwrap();
        assert!(!soul.has_frontmatter);
        assert_eq!(soul.config.name, None);
    }

    #[test]
    fn test_behavior_extra_fields() {
        let content = r#"---
behavior:
  proactive: true
  custom_flag: 42
---

Body.
"#;
        let soul = SoulFile::parse(content).unwrap();
        assert_eq!(soul.config.behavior.proactive, Some(true));
        assert!(soul.config.behavior.extra.contains_key("custom_flag"));
    }
}
