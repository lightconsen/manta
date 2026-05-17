//! Skill configuration management
//!
//! Handles user configuration for skills in ~/.manta/skills.json

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Skill configuration file
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillConfig {
    /// Per-skill configuration entries
    #[serde(default)]
    pub entries: HashMap<String, SkillEntryConfig>,
    /// Allowlist for bundled skills (empty = allow all)
    #[serde(rename = "allowBundled", default)]
    pub allow_bundled: Vec<String>,
    /// Installation preferences
    #[serde(default)]
    pub install: InstallConfig,
    /// Skill limits
    #[serde(default)]
    pub limits: SkillLimits,
}

/// Configuration for a specific skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEntryConfig {
    /// Whether the skill is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// API key configuration
    #[serde(default)]
    pub api_key: Option<ApiKeyConfig>,
    /// Environment variables to set
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Custom configuration
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

impl Default for SkillEntryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: None,
            env: HashMap::new(),
            config: HashMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// API key configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyConfig {
    pub source: String,   // "env" or "keychain"
    pub provider: String, // "default" or provider name
    pub id: String,       // env var name or keychain id
}

/// Installation preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallConfig {
    /// Prefer Homebrew on macOS
    #[serde(rename = "preferBrew", default = "default_true")]
    pub prefer_brew: bool,
    /// Node package manager preference
    #[serde(rename = "nodeManager", default = "default_node_manager")]
    pub node_manager: String,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            prefer_brew: true,
            node_manager: "npm".to_string(),
        }
    }
}

fn default_node_manager() -> String {
    "npm".to_string()
}

/// Skill limits for token optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillLimits {
    /// Maximum skills to include in prompt
    #[serde(rename = "maxSkillsInPrompt", default = "default_max_skills")]
    pub max_skills_in_prompt: usize,
    /// Maximum characters for skills prompt
    #[serde(rename = "maxSkillsPromptChars", default = "default_max_chars")]
    pub max_skills_prompt_chars: usize,
    /// Maximum skill file size in bytes
    #[serde(rename = "maxSkillFileBytes", default = "default_max_file_bytes")]
    pub max_skill_file_bytes: usize,
}

impl Default for SkillLimits {
    fn default() -> Self {
        Self {
            max_skills_in_prompt: 150,
            max_skills_prompt_chars: 30_000,
            max_skill_file_bytes: 256_000,
        }
    }
}

fn default_max_skills() -> usize {
    150
}

fn default_max_chars() -> usize {
    30_000
}

fn default_max_file_bytes() -> usize {
    256_000
}

impl SkillConfig {
    /// Load configuration from file
    pub async fn load() -> crate::Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            // Return default config
            return Ok(Self::default());
        }

        let content = tokio::fs::read_to_string(&config_path).await?;
        let config: Self = serde_json::from_str(&content)?;

        Ok(config)
    }

    /// Save configuration to file
    pub async fn save(&self) -> crate::Result<()> {
        let config_path = Self::config_path()?;

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(&config_path, content).await?;

        Ok(())
    }

    /// Get the configuration file path
    fn config_path() -> crate::Result<PathBuf> {
        // Use centralized ~/.manta directory
        Ok(crate::dirs::config_dir().join("skills.json"))
    }

    /// Check if a bundled skill is allowed
    pub fn is_bundled_allowed(&self, skill_name: &str) -> bool {
        // If allowlist is empty, allow all bundled skills
        if self.allow_bundled.is_empty() {
            return true;
        }

        self.allow_bundled.contains(&skill_name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_config_default() {
        let config = SkillConfig::default();
        assert!(config.entries.is_empty());
        assert!(config.allow_bundled.is_empty());
        assert!(config.is_bundled_allowed("any_skill"));
    }

    #[test]
    fn test_skill_entry_config_default() {
        let entry = SkillEntryConfig::default();
        assert!(entry.enabled);
        assert!(entry.api_key.is_none());
        assert!(entry.env.is_empty());
        assert!(entry.config.is_empty());
    }

    #[test]
    fn test_install_config_default() {
        let install = InstallConfig::default();
        assert!(install.prefer_brew);
        assert_eq!(install.node_manager, "npm");
    }

    #[test]
    fn test_skill_limits_default() {
        let limits = SkillLimits::default();
        assert_eq!(limits.max_skills_in_prompt, 150);
        assert_eq!(limits.max_skills_prompt_chars, 30_000);
        assert_eq!(limits.max_skill_file_bytes, 256_000);
    }

    #[test]
    fn test_is_bundled_allowed_with_allowlist() {
        let config = SkillConfig {
            allow_bundled: vec!["git".to_string(), "docker".to_string()],
            ..Default::default()
        };
        assert!(config.is_bundled_allowed("git"));
        assert!(config.is_bundled_allowed("docker"));
        assert!(!config.is_bundled_allowed("unknown"));
    }

    #[test]
    fn test_skill_config_serde_roundtrip() {
        let config = SkillConfig {
            entries: {
                let mut m = HashMap::new();
                m.insert(
                    "git".to_string(),
                    SkillEntryConfig {
                        enabled: true,
                        api_key: Some(ApiKeyConfig {
                            source: "env".to_string(),
                            provider: "openai".to_string(),
                            id: "OPENAI_API_KEY".to_string(),
                        }),
                        env: HashMap::new(),
                        config: HashMap::new(),
                    },
                );
                m
            },
            allow_bundled: vec!["git".to_string()],
            install: InstallConfig::default(),
            limits: SkillLimits::default(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let decoded: SkillConfig = serde_json::from_str(&json).unwrap();
        assert!(decoded.entries.contains_key("git"));
        assert_eq!(decoded.allow_bundled, vec!["git"]);
    }

    #[test]
    fn test_skill_config_serde_defaults() {
        let json = r#"{}"#;
        let config: SkillConfig = serde_json::from_str(json).unwrap();
        assert!(config.entries.is_empty());
        assert!(config.allow_bundled.is_empty());
        assert!(config.is_bundled_allowed("anything"));
    }

    #[test]
    fn test_skill_entry_config_serde_disabled() {
        let json = r#"{"enabled": false}"#;
        let entry: SkillEntryConfig = serde_json::from_str(json).unwrap();
        assert!(!entry.enabled);
    }

}