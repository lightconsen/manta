//! Dependency check results and execution chains.
//!
//! [`DependencyCheckResult`] and [`VersionMismatch`] report whether a skill's
//! dependencies are satisfied; [`SkillChain`] is an ordered runnable sequence
//! of skills.

use super::Skill;

/// Result of a dependency check
#[derive(Debug, Clone)]
pub struct DependencyCheckResult {
    pub satisfied: bool,
    pub missing: Vec<String>,
    pub version_mismatches: Vec<VersionMismatch>,
}

/// A version mismatch between required and found
#[derive(Debug, Clone)]
pub struct VersionMismatch {
    pub skill: String,
    pub required: String,
    pub found: String,
}

/// An execution chain of skills
#[derive(Debug, Clone)]
pub struct SkillChain {
    pub skills: Vec<Skill>,
    pub trigger_skill: String,
}

impl SkillChain {
    /// Get the number of skills in the chain
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Get the combined prompt for all skills
    pub fn to_combined_prompt(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("# Skill Chain: {}\n\n", self.trigger_skill));

        for (i, skill) in self.skills.iter().enumerate() {
            output.push_str(&format!("## Step {}: {}\n\n", i + 1, skill.name));
            output.push_str(&skill.to_prompt_section(None));
            output.push_str("\n\n---\n\n");
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_chain_empty() {
        let chain = SkillChain {
            skills: vec![],
            trigger_skill: "test".to_string(),
        };
        assert!(chain.is_empty());
        assert_eq!(chain.len(), 0);
    }

    #[test]
    fn test_skill_chain_combined_prompt() {
        let chain = SkillChain {
            skills: vec![
                Skill::new("step1", "First", "Do first thing"),
                Skill::new("step2", "Second", "Do second thing"),
            ],
            trigger_skill: "pipeline".to_string(),
        };
        let prompt = chain.to_combined_prompt();
        assert!(prompt.contains("Skill Chain: pipeline"));
        assert!(prompt.contains("Step 1: step1"));
        assert!(prompt.contains("Step 2: step2"));
    }

    #[test]
    fn test_dependency_check_result_satisfied() {
        let result = DependencyCheckResult {
            satisfied: true,
            missing: vec![],
            version_mismatches: vec![],
        };
        assert!(result.satisfied);
    }

    #[test]
    fn test_dependency_check_result_missing() {
        let result = DependencyCheckResult {
            satisfied: false,
            missing: vec!["missing-skill".to_string()],
            version_mismatches: vec![],
        };
        assert!(!result.satisfied);
        assert_eq!(result.missing.len(), 1);
    }
}
