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

pub(crate) mod comparison;
pub(crate) mod dataset;
pub(crate) mod harness;
pub(crate) mod loader;
pub(crate) mod rca;
pub(crate) mod scorer;
pub(crate) mod action;
pub(crate) mod standalone;

pub use dataset::{
    EvalSuite, EvalTask, EvalTaskSource, SuiteCategory, SkillEvalDesign,
    TriggerCase, ExecutionCase, QualityCase, ResilienceCase,
    FailureMode, DegradeExpectation, ParamMatcher,
};
pub use harness::{
    EvalHarness, TrialResult, ToolCallSummary, EvalSummary,
};
pub use loader::{list_suites, load_suite, load_tasks, default_evals_dir};
pub use comparison::{
    compare_versions, VersionComparison, ComparisonVerdict, extract_trial_results,
};
pub use rca::{
    RcaPipeline, RcaInput, RcaResult, RcaKnowledgeBase, RcaKnowledgeBaseEntry,
    ProblemPhenomenon, CandidateModule, BadcaseEntry, ModuleVerdict,
};
pub use scorer::{
    LayeredScorer, ScoringOutput, ScreeningLayer, Verdict,
};
pub use action::{
    ActionItem, ActionLevel, ImpactScope, Priority,
};
