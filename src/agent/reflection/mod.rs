//! Trajectory reflection — nudge-based background pattern discovery.
//!
//! The [`NudgeEngine`] runs as a background task every N turns, reviewing the
//! last M turns of conversation and writing interaction patterns to memory.
//! Non-blocking, trajectory-aware.

pub mod config;
pub mod critic;
pub mod nudge;
pub mod trajectory;
pub mod types;

pub use config::{NudgeConfig, ReflectionConfig};
pub use critic::Critic;
pub use nudge::{NudgeEngine, NudgeResult};
pub use trajectory::{Trajectory, TrajectoryStep, TrajectoryWindow};
pub use types::{Critique, QualityCriteria, QualityDimension};
