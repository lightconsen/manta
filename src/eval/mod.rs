//! Syscity Evaluation Framework
//!
//! Implements the three-tier evaluation methodology from the Agent 评测 article:
//!
//! - **Eval Harness** — N-trial execution engine with GoalCondition (Code Scorer)
//!   and Critic (LLM Judge) scoring, plus Wilson CI statistics.
//! - **RCA Pipeline** — Root cause analysis with ProblemPhenomenon × CandidateModule
//!   mapping table, 5-step investigation flow.
//! - **Quality Gates** — Gateway lifecycle integration for pre-release gating.
//!
//! # Quick Start
//!
//! ```rust
//! use syscity::eval::{EvalHarness, EvalTask, EvalTaskSource, QualityCriteria};
//!
//! # async fn example(agent: Arc<Agent>, critic: Critic) -> Result<()> {
//! let task = EvalTask {
//!     id: "example_task".into(),
//!     input: "Hello".into(),
//!     ..Default::default()
//! };
//! let harness = EvalHarness::new(agent, critic);
//! let summary = harness.run(task, 5).await?;
//! println!("Pass rate: {:.1}%", summary.pass_rate * 100.0);
//! # Ok(())
//! # }
//! ```

pub(crate) mod action;
pub(crate) mod agent_type;
pub(crate) mod comparison;
pub(crate) mod dataset;
pub(crate) mod harness;
pub(crate) mod loader;
pub(crate) mod rca;
pub(crate) mod recycle;
pub(crate) mod scorer;
pub(crate) mod skill_scorer;
pub(crate) mod standalone;

pub(crate) mod calibration;
pub(crate) mod human_review;
pub(crate) mod multi_judge;

pub use action::{ActionItem, ActionLevel, ImpactScope, Priority};
pub use agent_type::AgentType;
pub use calibration::{
    calibrate, detect_drift, load_calibration_cases, load_calibration_history,
    save_calibration_report, CalibrationCase, CalibrationReport, CalibrationResult,
};
pub use comparison::{
    compare_versions, extract_trial_results, ComparisonVerdict, VersionComparison,
};
pub use dataset::TurnInput;
pub use dataset::{
    DegradeExpectation, EvalSuite, EvalTask, EvalTaskSource, ExecutionCase, FailureMode,
    ParamMatcher, QualityCase, ResilienceCase, SkillEvalDesign, SuiteCategory, TriggerCase,
};
pub use harness::{EvalHarness, EvalSummary, ToolCallSummary, TrialResult, TurnResult};
pub use human_review::{HumanReviewCase, HumanReviewStore, ReviewStatus};
pub use loader::{default_evals_dir, list_suites, load_suite, load_tasks, LoadedTaskFile};
pub use multi_judge::{
    AggregatedResult, AggregatedVerdict, AggregationMode, JudgeConfig, JudgeResult,
    MultiJudgeConfig, MultiJudgeScorer,
};
pub use rca::{
    BadcaseEntry, CandidateModule, ModuleVerdict, ProblemPhenomenon, RcaInput, RcaKnowledgeBase,
    RcaKnowledgeBaseEntry, RcaPipeline, RcaResult,
};
pub use recycle::{load_badcase_suite, BadcaseCluster, BadcaseCollector, BadcaseFixStatus, BadcaseRecord};
pub use scorer::{LayeredScorer, RiskSignalChecker, ScoringOutput, ScreeningLayer, Verdict};
pub use skill_scorer::{
    ExecutionCheckResult, QualityCheckResult, ResilienceCheckResult, SkillCheckResult, SkillScorer,
    TriggerCheckResult,
};
