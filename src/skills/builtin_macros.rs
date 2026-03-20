//! Macros for loading built-in skills at compile time
//!
//! This module provides compile-time skill loading from SKILL.md files,
//! replacing the previous approach of embedding skills as Rust constants.

/// Include all built-in skills as a HashMap
///
/// This macro loads SKILL.md files from the builtin directory at compile time
/// and returns a HashMap of skill name to skill content.
#[macro_export]
macro_rules! include_builtin_skills {
    () => {{
        let mut skills: std::collections::HashMap<&'static str, &'static str> =
            std::collections::HashMap::new();

        // Skill creator
        skills.insert("skill-creator", include_str!("builtin/skill-creator/SKILL.md"));

        // Find skills
        skills.insert("find-skills", include_str!("builtin/find-skills/SKILL.md"));

        // GitHub
        skills.insert("github", include_str!("builtin/github/SKILL.md"));

        // Weather
        skills.insert("weather", include_str!("builtin/weather/SKILL.md"));

        // TODO: Add remaining skills as they're converted
        // - cron
        // - clawhub
        // - summarize
        // - tmux
        // - agent-browser
        // - api-gateway
        // - nano-pdf
        // - self-improving-agent
        // - agent-creator

        skills
    }};
}

/// Parse all built-in skills and return a HashMap of name to Skill structs
#[macro_export]
macro_rules! load_builtin_skills {
    () => {{
        use $crate::skills::frontmatter::SkillFile;
        use $crate::skills::{Skill, SkillTrigger, StorageLevel, TriggerType};

        let skill_contents = $crate::include_builtin_skills!();
        let mut skills: std::collections::HashMap<String, Skill> = std::collections::HashMap::new();

        for (name, content) in skill_contents {
            let path = std::path::PathBuf::from(format!("builtin/{}/SKILL.md", name));
            match SkillFile::parse(content, path) {
                Ok(skill_file) => {
                    let mut skill = Skill::new(
                        skill_file.frontmatter.name.clone(),
                        skill_file.frontmatter.description.clone(),
                        skill_file.content.clone(),
                    );
                    skill.version = skill_file.frontmatter.version.clone();
                    skill.author = skill_file.frontmatter.author.clone();
                    skill.source_level = StorageLevel::Bundled;
                    skill.is_eligible = true;
                    skill.enabled = true;
                    skill.source_path =
                        std::path::PathBuf::from(format!("builtin/{}/SKILL.md", name));

                    // Convert frontmatter triggers to SkillTriggers
                    for trigger in &skill_file.frontmatter.triggers {
                        let trigger_type = match trigger.trigger_type.as_str() {
                            "command" => TriggerType::Command,
                            "keyword" => TriggerType::Keyword,
                            "regex" => TriggerType::Regex,
                            "intent" => TriggerType::Intent,
                            _ => TriggerType::Keyword,
                        };
                        skill.triggers.push(SkillTrigger {
                            trigger_type,
                            pattern: trigger.pattern.clone(),
                            priority: trigger.priority,
                            user_invocable: true,
                            model_invocable: true,
                        });
                    }

                    // Use openclaw emoji if available, fall back to legacy emoji
                    if !skill_file.frontmatter.openclaw.emoji.is_empty() {
                        skill.metadata.emoji = skill_file.frontmatter.openclaw.emoji.clone();
                    } else if !skill_file.frontmatter.emoji.is_empty() {
                        skill.metadata.emoji = skill_file.frontmatter.emoji.clone();
                    }

                    skills.insert(name.to_string(), skill);
                }
                Err(e) => {
                    tracing::warn!("Failed to parse built-in skill '{}': {}", name, e);
                }
            }
        }

        skills
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_skills_macro() {
        let skills = include_builtin_skills!();
        assert!(skills.contains_key("skill-creator"));
        assert!(skills.contains_key("find-skills"));
        assert!(skills.contains_key("github"));
        assert!(skills.contains_key("weather"));
    }
}
