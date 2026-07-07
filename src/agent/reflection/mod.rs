//! Trajectory reflection — retrospect-based background pattern discovery.
//!
//! The [`RetrospectEngine`] runs as a background task every N turns, reviewing
//! the last M turns of conversation and writing interaction patterns to memory.
//! Non-blocking, trajectory-aware.

pub mod config;
pub mod critic;
pub mod retrospect;
pub mod trajectory;
pub mod types;

pub use config::{ReflectionConfig, RetrospectConfig};
pub use critic::Critic;
pub use retrospect::{RetrospectEngine, RetrospectResult};
pub use trajectory::{Trajectory, TrajectoryStep, TrajectoryWindow};
pub use types::{Critique, QualityCriteria, QualityDimension};
