//! ACP (Agent Control Plane) — Re-export module
//!
//! All ACP types and functionality have been unified into `crate::acp`.
//! This module provides backward-compatible re-exports.

pub use crate::acp::{
    AcpCommand, AcpControlPlane, AcpSessionStatus, ExecutionController, ExecutionMode, RuntimeState,
};

/// Backward-compatible alias for `AcpControlPlane`.
pub type AcpController = crate::acp::AcpControlPlane;
