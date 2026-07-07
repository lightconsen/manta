//! Reflection pattern — self-critique and iterative improvement of agent output.
//!
//! Implements the Reflection Pattern from *Agentic Design Patterns* (Ch.4).
//! After the agent produces a response, the [`ReflectionPipeline`] evaluates
//! it against quality criteria using an LLM critic. If the output falls below
//! thresholds, the critic provides structured feedback and the agent generates
//! an improved version. This loop continues until the output passes or the
//! iteration budget is exhausted.
//!
//! ## Architecture
//!
//! ```text
//! Agent output → Critic (LLM judge) → Pass? → Yes → Return output
//!                                      ↓ No
//!                              Improve (LLM regenerate) → loop
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! let pipeline = ReflectionPipeline::new(config, provider);
//! if pipeline.should_trigger(&user_msg, &response) {
//!     let result = pipeline.reflect(&content, &user_msg, &[]).await;
//!     // result.final_content contains the improved response
//! }
//! ```

pub mod config;
pub mod critic;
pub mod pipeline;
pub mod types;

pub use config::{ReflectionConfig, ReflectionTrigger};
pub use critic::Critic;
pub use pipeline::{ReflectionPipeline, ReflectionResult};
pub use types::{Critique, QualityCriteria, QualityDimension, ReflectionTarget};
