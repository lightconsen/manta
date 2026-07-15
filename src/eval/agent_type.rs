//! Agent types — evaluation focus differentiation per §02.
//!
//! Each [`AgentType`] variant carries metadata used to adjust scoring emphasis
//! and evaluation criteria for different agent capability domains.

use serde::{Deserialize, Serialize};

/// Type of agent capability being evaluated (§02).
///
/// Each variant expresses a different evaluation focus — factual accuracy for
/// knowledge QA, tool correctness for task execution, reasoning quality for
/// decision agents, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentType {
    /// 知识问答型 — factual recall, citation faithfulness, hallucination avoidance.
    #[serde(rename = "knowledge_qa")]
    KnowledgeQA,
    /// 任务执行型 — multi-step tool orchestration, instruction following, error recovery.
    #[serde(rename = "task_execution")]
    TaskExecution,
    /// 推理决策型 — logical reasoning, chain-of-thought, trade-off analysis.
    #[serde(rename = "reasoning_decision")]
    ReasoningDecision,
    /// 多轮引导型 — sustained context, persona consistency, multi-turn coherence.
    #[serde(rename = "multi_turn_guide")]
    MultiTurnGuide,
    /// 创意生成型 — structured output formats, creativity constraints.
    #[serde(rename = "creative_generation")]
    CreativeGeneration,
    /// 多 Agent 协作型 — sub-agent coordination, role separation, message passing.
    #[serde(rename = "multi_agent")]
    MultiAgent,
}

impl AgentType {
    /// Human-readable label in Chinese.
    pub fn label(&self) -> &'static str {
        match self {
            Self::KnowledgeQA => "知识问答型",
            Self::TaskExecution => "任务执行型",
            Self::ReasoningDecision => "推理决策型",
            Self::MultiTurnGuide => "多轮引导型",
            Self::CreativeGeneration => "创意生成型",
            Self::MultiAgent => "多Agent协作型",
        }
    }

    /// English label for internal use.
    pub fn label_en(&self) -> &'static str {
        match self {
            Self::KnowledgeQA => "Knowledge QA",
            Self::TaskExecution => "Task Execution",
            Self::ReasoningDecision => "Reasoning & Decision",
            Self::MultiTurnGuide => "Multi-turn Guide",
            Self::CreativeGeneration => "Creative Generation",
            Self::MultiAgent => "Multi-Agent Collaboration",
        }
    }

    /// Scoring emphasis string injected into the Critic's system prompt.
    ///
    /// This guides the LLM Judge to focus on the most relevant quality
    /// dimensions for this agent type.
    pub fn scoring_emphasis(&self) -> &'static str {
        match self {
            Self::KnowledgeQA => {
                "Focus on factual accuracy and citation faithfulness. \
                 Penalize hallucination, guesswork, or making up information. \
                 Verify that claims are supported by the retrieved context."
            }
            Self::TaskExecution => {
                "Focus on tool selection correctness, parameter accuracy, and \
                 error recovery. Penalize unnecessary tool calls, incorrect \
                 parameters, or failure to handle tool errors gracefully."
            }
            Self::ReasoningDecision => {
                "Focus on logical soundness, evidence use, and trade-off \
                 awareness. Penalize circular reasoning, unsupported \
                 conclusions, or ignoring contradictory evidence."
            }
            Self::MultiTurnGuide => {
                "Focus on context retention, persona consistency, and \
                 progressive understanding across turns. Penalize forgetting \
                 earlier context, inconsistent responses, or failing to \
                 track user preferences. Additionally evaluate emotion and \
                 sentiment handling — does the agent acknowledge user \
                 frustration, excitement, or hesitation? Also detect goal \
                 switching adaptability — when the user changes topic, does \
                 the agent smoothly transition without losing previous context?"
            }
            Self::CreativeGeneration => {
                "Focus on format compliance, creativity within constraints, \
                 and structural completeness. Penalize generic output, \
                 format violations, or incomplete responses."
            }
            Self::MultiAgent => {
                "Focus on sub-agent delegation correctness, role separation, \
                 and inter-agent message fidelity. Penalize incorrect routing, \
                 role confusion, or information loss between agents."
            }
        }
    }

    /// Evaluation focus description for reporting.
    pub fn eval_focus(&self) -> &'static str {
        match self {
            Self::KnowledgeQA => "Accuracy, Citation Faithfulness, Hallucination Avoidance",
            Self::TaskExecution => "Tool Selection, Parameter Accuracy, Error Recovery",
            Self::ReasoningDecision => "Logical Soundness, Evidence Use, Trade-off Analysis",
            Self::MultiTurnGuide => "Context Retention, Emotion Handling, Goal Switching, Persona Consistency",
            Self::CreativeGeneration => "Format Compliance, Creativity, Structural Completeness",
            Self::MultiAgent => "Sub-agent Delegation, Role Separation, Message Fidelity",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_variants_have_labels() {
        for variant in &[
            AgentType::KnowledgeQA,
            AgentType::TaskExecution,
            AgentType::ReasoningDecision,
            AgentType::MultiTurnGuide,
            AgentType::CreativeGeneration,
            AgentType::MultiAgent,
        ] {
            assert!(!variant.label().is_empty());
            assert!(!variant.label_en().is_empty());
            assert!(!variant.scoring_emphasis().is_empty());
            assert!(!variant.eval_focus().is_empty());
        }
    }

    #[test]
    fn test_serde_roundtrip() {
        for variant in &[
            AgentType::KnowledgeQA,
            AgentType::TaskExecution,
            AgentType::ReasoningDecision,
            AgentType::MultiTurnGuide,
            AgentType::CreativeGeneration,
            AgentType::MultiAgent,
        ] {
            let json = serde_json::to_string(variant).unwrap();
            let deserialized: AgentType = serde_json::from_str(&json).unwrap();
            assert_eq!(*variant, deserialized);
        }
    }

    #[test]
    fn test_serde_snake_case() {
        // Verify the serde rename works
        let json = "\"knowledge_qa\"";
        let parsed: AgentType = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, AgentType::KnowledgeQA);
    }
}
