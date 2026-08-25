//! Built-in skills for Syscity
//!
//! These skills are always available and cannot be uninstalled.
//! They provide core functionality for skill management and scheduling.
//!
//! This module delegates to the `load_builtin_skills!()` macro which
//! parses SKILL.md files at compile time, eliminating the previous
//! dual-source-of-truth between inline Rust constants and SKILL.md files.

use std::collections::HashMap;

use super::Skill;

/// Get all built-in skills.
///
/// Skills are loaded from their SKILL.md files at compile time via
/// the `load_builtin_skills!()` macro.  Minimal post-processing is
/// applied for properties that cannot be expressed in the SKILL.md
/// frontmatter alone (e.g., nano-pdf's required binaries).
pub fn get_builtin_skills() -> HashMap<String, Skill> {
    let mut skills = crate::load_builtin_skills!();

    // nano-pdf requires pdftotext and pandoc; the SKILL.md frontmatter
    // does not yet declare these, so apply them here.
    if let Some(skill) = skills.get_mut("nano-pdf") {
        skill.metadata.requires.bins.push("pdftotext".to_string());
        skill.metadata.requires.bins.push("pandoc".to_string());
    }

    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_skills_created() {
        let skills = get_builtin_skills();
        assert!(skills.contains_key("skill-creator"));
        assert!(skills.contains_key("find-skills"));
        assert!(skills.contains_key("cron"));
        assert!(skills.contains_key("clawhub"));
        assert!(skills.contains_key("summarize"));
        assert!(skills.contains_key("weather"));
        assert!(skills.contains_key("tmux"));
        assert!(skills.contains_key("github"));
        assert!(skills.contains_key("agent-browser"));
        assert!(skills.contains_key("api-gateway"));
        assert!(skills.contains_key("nano-pdf"));
        assert!(skills.contains_key("self-improving-agent"));
        assert!(skills.contains_key("agent-creator"));
        assert!(skills.contains_key("document-authoring"));
        assert_eq!(skills.len(), 14);
    }

    #[test]
    fn test_skill_creator_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("skill-creator").unwrap();

        assert_eq!(skill.name, "skill-creator");
        assert_eq!(skill.metadata.emoji, "🛠️");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_find_skills_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("find-skills").unwrap();

        assert_eq!(skill.name, "find-skills");
        assert_eq!(skill.metadata.emoji, "🔍");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
    }

    #[test]
    fn test_cron_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("cron").unwrap();

        assert_eq!(skill.name, "cron");
        assert_eq!(skill.metadata.emoji, "⏰");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
    }

    #[test]
    fn test_clawhub_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("clawhub").unwrap();

        assert_eq!(skill.name, "clawhub");
        assert_eq!(skill.metadata.emoji, "🦞");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_summarize_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("summarize").unwrap();

        assert_eq!(skill.name, "summarize");
        assert_eq!(skill.metadata.emoji, "📋");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_weather_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("weather").unwrap();

        assert_eq!(skill.name, "weather");
        assert_eq!(skill.metadata.emoji, "🌤️");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_tmux_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("tmux").unwrap();

        assert_eq!(skill.name, "tmux");
        assert_eq!(skill.metadata.emoji, "🖥️");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_github_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("github").unwrap();

        assert_eq!(skill.name, "github");
        assert_eq!(skill.metadata.emoji, "🐙");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_agent_browser_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("agent-browser").unwrap();

        assert_eq!(skill.name, "agent-browser");
        assert_eq!(skill.metadata.emoji, "🌐");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_api_gateway_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("api-gateway").unwrap();

        assert_eq!(skill.name, "api-gateway");
        assert_eq!(skill.metadata.emoji, "🔌");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_nano_pdf_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("nano-pdf").unwrap();

        assert_eq!(skill.name, "nano-pdf");
        assert_eq!(skill.metadata.emoji, "📄");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
        // Nano PDF requires pdftotext and pandoc
        assert!(skill
            .metadata
            .requires
            .bins
            .contains(&"pdftotext".to_string()));
        assert!(skill.metadata.requires.bins.contains(&"pandoc".to_string()));
    }

    #[test]
    fn test_self_improving_agent_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("self-improving-agent").unwrap();

        assert_eq!(skill.name, "self-improving-agent");
        assert_eq!(skill.metadata.emoji, "🔄");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }

    #[test]
    fn test_agent_creator_properties() {
        let skills = get_builtin_skills();
        let skill = skills.get("agent-creator").unwrap();

        assert_eq!(skill.name, "agent-creator");
        assert_eq!(skill.metadata.emoji, "🤖");
        assert!(skill.is_eligible);
        assert!(skill.enabled);
        assert!(!skill.triggers.is_empty());
    }
}
