//! Feishu Channel Implementation
//!
//! This module re-exports from the Lark module since Feishu and Lark
//! are the same platform (Feishu = China version, Lark = International).
//!
//! Both use the same ByteDance Open Platform API.

pub use super::lark::*;

// Alias Lark types as Feishu types for API consistency
pub type FeishuConfig = LarkConfig;
pub type FeishuChannel = LarkChannel;
