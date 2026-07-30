//! Gateway request/response DTOs and REST API types.
//!
//! Extracted from `gateway/mod.rs` to reduce the main control-plane file.
//! Re-exported via `pub use types::*;` so all existing import paths
//! (`crate::gateway::HealthReport`, etc.) continue to work.

use serde::{Deserialize, Serialize};

// ── WebSocket ────────────────────────────────────────────────────────────────

/// Query parameters for WebSocket connection
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    /// Start a new conversation (true/false)
    pub new: Option<bool>,
    /// Specific conversation ID to resume
    pub conversation: Option<String>,
}

// ── Provider switch ──────────────────────────────────────────────────────────

/// Request body for switching default model
#[derive(Debug, Deserialize)]
pub struct SwitchModelRequest {
    /// Model alias to switch to (e.g., "fast", "smart", "default")
    pub model: String,
}

// ── Send message ─────────────────────────────────────────────────────────────

/// Request body for provider override in messages
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    /// Message content
    pub message: String,
    /// Optional caller user ID (falls back to "api_user")
    pub user_id: Option<String>,
    /// Optional provider override (e.g., "anthropic", "openai")
    pub provider_override: Option<String>,
    /// Optional model alias override (e.g., "fast", "smart")
    pub model_alias: Option<String>,
    /// Optional specific model ID override
    pub model_id: Option<String>,
}

// ── Health-report DTOs ───────────────────────────────────────────────────────

/// Health report response structure
#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub status: String,
    pub version: String,
    pub timestamp: String,
    pub overall_healthy: bool,
    pub subsystems: SubsystemHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dream: Option<DreamHealthReport>,
}

/// Dream observability report embedded in the health endpoint.
#[derive(Debug, Serialize)]
pub struct DreamHealthReport {
    pub dreams_total: u64,
    pub dreams_failed: u64,
    pub memories_processed_total: u64,
    pub memories_created_total: u64,
    pub memories_removed_total: u64,
    pub memories_promoted_total: u64,
    pub memories_demoted_total: u64,
    pub dream_duration_ms_total: u64,
    pub llm_tokens_input_total: u64,
    pub llm_tokens_output_total: u64,
}

/// Per-subsystem health statuses
#[derive(Debug, Serialize)]
pub struct SubsystemHealth {
    pub agents: HealthStatus,
    pub providers: HealthStatus,
    pub channels: HealthStatus,
    #[serde(rename = "vector_memory")]
    pub vector_memory: HealthStatus,
    #[serde(rename = "memory_manager")]
    pub memory_manager: HealthStatus,
    pub cron: HealthStatus,
    pub plugins: HealthStatus,
    pub mcp: HealthStatus,
    pub storage: HealthStatus,
    #[serde(rename = "cost_guard")]
    pub cost_guard: HealthStatus,
}

/// Individual subsystem health status
#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub healthy: bool,
    pub message: String,
}

// ── Chat compatibility ───────────────────────────────────────────────────────

/// Simple chat handler for backwards compatibility with DaemonClient
#[derive(Debug, Deserialize)]
pub struct ChatRequestCompat {
    pub message: String,
    pub conversation_id: Option<String>,
}

/// Request body for web terminal chat
#[derive(Debug, Deserialize)]
pub struct WebTerminalChatRequest {
    /// Message content from user
    pub message: String,
    /// Optional conversation ID (creates new if not provided)
    pub conversation_id: Option<String>,
    /// Optional user ID
    pub user_id: Option<String>,
}

/// Response for web terminal chat
#[derive(Debug, Serialize)]
pub struct WebTerminalChatResponse {
    /// Message ID
    pub message_id: String,
    /// Conversation ID (new or existing)
    pub conversation_id: String,
    /// Status
    pub status: String,
}

// ── Provider fallback chain ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetFallbackChainRequest {
    pub providers: Vec<String>,
}

// ── Vector Memory API ────────────────────────────────────────────────────────

fn default_memory_limit() -> usize {
    10
}

fn default_memory_threshold() -> f32 {
    0.7
}

#[derive(Debug, Deserialize)]
pub struct MemorySearchRequest {
    pub query: String,
    #[serde(default = "default_memory_limit")]
    pub limit: usize,
    #[serde(default)]
    pub collection: String,
    #[serde(default = "default_memory_threshold")]
    pub threshold: f32,
}

#[derive(Debug, Deserialize)]
pub struct MemoryAddRequest {
    pub content: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    #[serde(default)]
    pub collection: String,
}

// ── Skill runner ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RunSkillRequest {
    /// Input for the skill
    pub input: String,
}

// ── MCP ──────────────────────────────────────────────────────────────────────

fn mcp_default_timeout() -> u64 {
    120
}

/// Request body for connecting an MCP server
#[derive(Debug, Deserialize)]
pub struct McpConnectRequest {
    #[serde(default)]
    pub transport: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    pub url: Option<String>,
    #[serde(default = "mcp_default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub max_tools: usize,
    /// OAuth / bearer auth configuration
    pub auth_type: Option<String>,
    pub client_id: Option<String>,
    pub auth_url: Option<String>,
    pub token_url: Option<String>,
    pub scopes: Option<String>,
}

/// Request body for reading a resource
#[derive(Debug, Deserialize)]
pub struct McpReadResourceRequest {
    pub uri: String,
}

// ── OpenAI-compatible API
// ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OpenAiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    pub stream: bool,
}

/// Query parameters for model override.
#[derive(Debug, Deserialize)]
pub struct ModelOverrideQuery {
    #[serde(rename = "model")]
    pub model: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAiChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<OpenAiChoice>,
    pub usage: OpenAiUsage,
}

#[derive(Debug, Serialize)]
pub struct OpenAiChoice {
    pub index: u32,
    pub message: OpenAiResponseMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAiResponseMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAiUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ── Runtime settings CRUD ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetSettingRequest {
    pub key: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct DenyApprovalRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddCronJobRequest {
    pub name: String,
    pub schedule: String,
    pub command: String,
}

// ── Mention Gate ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetMentionPolicyRequest {
    pub policy: crate::security::mention_gate::MentionPolicy,
}

#[derive(Debug, Deserialize)]
pub struct AddMentionPatternRequest {
    pub channel: String,
    pub pattern: String,
}
