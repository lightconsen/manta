//! Syscity WebSocket Protocol
//!
//! Implements the WebSocket-native RPC protocol defined in docs/protocol.md.
//! Uses req/res/event framing aligned with

use crate::gateway::GatewayEvent;
use crate::security::UserId;
use serde::{Deserialize, Serialize};
// ── Protocol Version ──────────────────────────────────────────────────────────

/// Current protocol version
pub const PROTOCOL_VERSION: u32 = 1;

/// Minimum protocol version supported by this server
pub const PROTOCOL_VERSION_MIN: u32 = 1;

// ── Frame Types ───────────────────────────────────────────────────────────────

/// A frame received from the client (always a request)
#[derive(Debug, Clone, Deserialize)]
pub struct WsRequest {
 /// Frame type discriminator — always "req" for client messages
    #[serde(rename = "type")]
    pub frame_type: String,
 /// Client-generated request ID (mirrored in response)
    pub id: String,
 /// Method name, dot-namespaced (e.g. "chat.send")
    pub method: String,
 /// Method-specific parameters
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// A response frame sent to the client
#[derive(Debug, Clone, Serialize)]
pub struct WsResponse {
 /// Frame type discriminator — always "res"
    #[serde(rename = "type")]
    pub frame_type: &'static str,
 /// Mirrors the request ID
    pub id: String,
 /// Success flag
    pub ok: bool,
 /// Response payload (when ok = true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
 /// Error details (when ok = false)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WsError>,
}

impl WsResponse {
 /// Build a successful response
    pub fn ok(id: impl Into<String>, payload: impl Serialize) -> Self {
        Self {
            frame_type: "res",
            id: id.into(),
            ok: true,
            payload: serde_json::to_value(payload).ok(),
            error: None,
        }
    }

 /// Build an error response
    pub fn err(id: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            frame_type: "res",
            id: id.into(),
            ok: false,
            payload: None,
            error: Some(WsError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

/// An event frame pushed from server to client
#[derive(Debug, Clone, Serialize)]
pub struct WsEvent {
 /// Frame type discriminator — always "event"
    #[serde(rename = "type")]
    pub frame_type: &'static str,
 /// Event name (e.g. "chat.delta")
    pub event: String,
 /// Event-specific payload
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
 /// Monotonic sequence number for ordering/dedup
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

impl WsEvent {
 /// Build an event frame
    pub fn new(event: impl Into<String>, payload: impl Serialize, seq: u64) -> Self {
        Self {
            frame_type: "event",
            event: event.into(),
            payload: serde_json::to_value(payload).ok(),
            seq: Some(seq),
        }
    }
}

/// Error shape in a response frame
#[derive(Debug, Clone, Serialize)]
pub struct WsError {
 /// Error code (e.g. "UNAUTHORIZED", "SESSION_NOT_FOUND")
    pub code: String,
 /// Human-readable error message
    pub message: String,
}

// ── Connect Handshake ─────────────────────────────────────────────────────────

/// Parameters sent by client in the first `connect` request
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectParams {
 /// Protocol version requested by client
    pub protocol_version: u32,
 /// Client identification
    pub client: Option<ClientInfo>,
 /// Authentication credentials
    pub auth: Option<AuthParams>,
 /// Device identity (for device pairing mode)
    pub device: Option<DeviceIdentity>,
 /// Requested scopes
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// Client identification
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClientInfo {
 /// Client type: "web", "ios", "android", "cli"
    pub id: String,
 /// Client software version
    #[serde(default)]
    pub version: String,
}

/// Authentication parameters within connect
#[derive(Debug, Clone, Deserialize)]
pub struct AuthParams {
 /// Shared token or device token
    #[serde(default)]
    pub token: Option<String>,
 /// Password (for password auth mode)
    #[serde(default)]
    pub password: Option<String>,
}

/// Device identity for pairing
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceIdentity {
 /// Device unique ID
    pub id: String,
 /// Ed25519 public key (base64)
    #[serde(default)]
    pub public_key: Option<String>,
 /// Signature over nonce + timestamp
    #[serde(default)]
    pub signature: Option<String>,
 /// Nonce from the connect challenge
    #[serde(default)]
    pub nonce: Option<String>,
}

/// Payload of the hello-ok response
#[derive(Debug, Clone, Serialize)]
pub struct HelloOkPayload {
 /// Protocol version accepted by server
    pub protocol_version: u32,
 /// Session key derived for this connection
    pub session_key: String,
 /// Available features / methods
    pub features: Vec<String>,
 /// Scopes granted to this connection
    pub scopes_granted: Vec<String>,
 /// Server info
    pub server: ServerInfo,
}

/// Server info in hello-ok
#[derive(Debug, Clone, Serialize)]
pub struct ServerInfo {
 /// Server version string
    pub version: String,
 /// Connection ID
    pub conn_id: String,
}

// ── Scopes (Operator Scope system) ────────────────────────────────────────────
//
// Syscity uses an Operator Scope model. Each WebSocket method declares a required scope;
// the gateway verifies the connection's granted scopes before dispatch.
//
// Scope hierarchy (least to most privileged):
// read < chat < write < acp < pairing < admin

// Scope: chat operations (send, history, abort)
pub const SCOPE_CHAT: &str = "chat";
/// Scope: read-only queries
pub const SCOPE_READ: &str = "read";
/// Scope: write/modify operations
pub const SCOPE_WRITE: &str = "write";
/// Scope: admin (full access)
pub const SCOPE_ADMIN: &str = "admin";
/// Scope: device pairing management
pub const SCOPE_PAIRING: &str = "pairing";
/// Scope: ACP (Agent Control Plane) operations
pub const SCOPE_ACP: &str = "acp";

/// All available scopes
pub const ALL_SCOPES: &[&str] = &[
    SCOPE_CHAT,
    SCOPE_READ,
    SCOPE_WRITE,
    SCOPE_ADMIN,
    SCOPE_PAIRING,
    SCOPE_ACP,
];

/// Default scopes granted when none are explicitly requested
pub const DEFAULT_SCOPES: &[&str] = &[SCOPE_CHAT, SCOPE_READ];

/// Check if a method requires a specific scope
pub fn method_scope(method: &str) -> Option<&'static str> {
    match method {
        "chat.send" | "chat.abort" => Some(SCOPE_CHAT),
        "chat.history" | "sessions.list" | "agents.list" | "agents.get" | "agents.registry"
        | "health" | "system.presence" | "commands.list" | "config.get" | "models.list"
        | "models.add" | "models.remove" | "models.set_default" | "cron.list" | "skills.list"
        | "skills.install" | "logs.subscribe" | "logs.unsubscribe" | "tasks.list" => Some(SCOPE_READ),
        "sessions.create"
        | "sessions.delete"
        | "sessions.reset"
        | "sessions.subscribe"
        | "sessions.unsubscribe"
        | "commands.execute"
        | "config.set"
        | "tasks.schedule"
        | "tasks.delete"
        | "tasks.enable"
        | "tasks.disable" => Some(SCOPE_WRITE),
        "acp.spawn"
        | "acp.terminate"
        | "acp.message"
        | "acp.pause"
        | "acp.resume"
        | "acp.step"
        | "acp.cancel"
        | "acp.execute.session"
        | "acp.execute.run" => Some(SCOPE_ACP),
        "acp.list" | "acp.status" | "acp.tree" => Some(SCOPE_READ),
        "connect" | "ping" => None, // No scope required
        _ => {
 // Admin scope required for unknown methods (default-deny)
            Some(SCOPE_ADMIN)
        }
    }
}

/// Check if granted scopes allow a method
pub fn scopes_allow(granted: &[String], method: &str) -> bool {
    if granted.contains(&SCOPE_ADMIN.to_string()) {
        return true;
    }

    let required = match method_scope(method) {
        Some(s) => s,
        None => return true, // No scope required
    };

    granted.contains(&required.to_string())
}

// ── Auth Mode ─────────────────────────────────────────────────────────────────

/// Gateway authentication mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthMode {
 /// No authentication (development only)
    #[serde(rename = "none")]
    #[default]
    None,
 /// Shared secret token
    #[serde(rename = "token")]
    Token,
 /// Device pairing required
    #[serde(rename = "device")]
    Device,
 /// Tailscale automatic auth
    #[serde(rename = "tailscale")]
    Tailscale,
}

// ── Connection State ──────────────────────────────────────────────────────────

/// Per-connection state maintained during the WebSocket session
#[derive(Debug)]
pub struct ProtocolConnection {
 /// Whether the handshake is complete
    pub handshaked: bool,
 /// Granted scopes
    pub scopes: Vec<String>,
 /// User ID if authenticated
    pub user_id: Option<UserId>,
 /// Client info
    pub client: Option<ClientInfo>,
 /// Subscribed session IDs (empty = all)
    pub subscriptions: Vec<String>,
 /// Monotonic sequence counter for events
    pub seq: u64,
 /// Connection ID
    pub conn_id: String,
 /// Log subscription cancel sender
    #[allow(dead_code)]
    pub log_cancel_tx: Option<tokio::sync::mpsc::Sender<()>>,
}

impl ProtocolConnection {
    pub fn new(conn_id: impl Into<String>) -> Self {
        Self {
            handshaked: false,
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
            user_id: None,
            client: None,
            subscriptions: Vec::new(),
            seq: 0,
            conn_id: conn_id.into(),
            log_cancel_tx: None,
        }
    }

 /// Increment and return the next sequence number
    pub fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

 /// Check if this connection is subscribed to a session
    pub fn is_subscribed(&self, session_id: &str) -> bool {
        self.subscriptions.is_empty() || self.subscriptions.contains(&session_id.to_string())
    }
}

// ── GatewayEvent → WsEvent Mapping ────────────────────────────────────────────

/// Convert a GatewayEvent to a WsEvent name + payload
pub fn gateway_event_to_ws(event: &GatewayEvent) -> Option<(String, serde_json::Value)> {
    match event {
        GatewayEvent::AgentResponse { .. } => {
 // Suppressed: non-streaming responses emit chat.final via Completed,
 // so emitting chat.delta here would duplicate the full content.
            None
        }
        GatewayEvent::Thinking { session_id, agent_id, content } => Some((
            "agent.thinking".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "content": content,
            }),
        )),
        GatewayEvent::ContentDelta { session_id, agent_id, delta } => Some((
            "chat.delta".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "content": delta,
            }),
        )),
        GatewayEvent::ToolCalling {
            session_id,
            agent_id,
            tool_name,
            arguments,
        } => Some((
            "tool.calling".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "tool_name": tool_name,
                "arguments": arguments,
            }),
        )),
        GatewayEvent::ToolResult {
            session_id,
            agent_id,
            tool_name,
            result,
            data,
        } => Some((
            "tool.result".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "tool_name": tool_name,
                "result": result,
                "data": data,
            }),
        )),
        GatewayEvent::Completed { session_id, agent_id, response } => Some((
            "chat.final".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "response": response,
            }),
        )),
        GatewayEvent::ProcessingError { session_id, agent_id, message } => Some((
            "chat.error".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "message": message,
            }),
        )),
        GatewayEvent::MessageReceived {
            channel,
            user_id,
            content,
            timestamp,
        } => Some((
            "message.received".to_string(),
            serde_json::json!({
                "channel": channel,
                "user_id": user_id,
                "content": content,
                "timestamp": timestamp,
            }),
        )),
        GatewayEvent::AgentStatus { agent_id, status } => Some((
            "agent.status".to_string(),
            serde_json::json!({
                "agent_id": agent_id,
                "status": format!("{:?}", status),
            }),
        )),
        GatewayEvent::ChannelStatus { channel, connected } => Some((
            "channel.status".to_string(),
            serde_json::json!({
                "channel": channel,
                "connected": connected,
            }),
        )),
        GatewayEvent::ApprovalRequired {
            approval_id,
            tool_name,
            requested_by,
            risk_level,
            message,
        } => Some((
            "approval.required".to_string(),
            serde_json::json!({
                "approval_id": approval_id,
                "tool_name": tool_name,
                "requested_by": requested_by,
                "risk_level": format!("{:?}", risk_level),
                "message": message,
            }),
        )),
        GatewayEvent::CronAnnounce { channel: _, to: _, message } => {
 // message is a JSON string produced by CronScheduler; try to parse it
            let payload = serde_json::from_str(message)
                .unwrap_or_else(|_| serde_json::json!({ "message": message }));
            Some(("cron.completed".to_string(), payload))
        }
        GatewayEvent::RepairAction {
            kind,
            target_id,
            description,
            restart_count,
        } => Some((
            "repair.action".to_string(),
            serde_json::json!({
                "kind": kind,
                "target_id": target_id,
                "description": description,
                "restart_count": restart_count,
            }),
        )),
        GatewayEvent::DevicePairRequested { device_id, code, display_name } => Some((
            "device.pair.requested".to_string(),
            serde_json::json!({
                "device_id": device_id,
                "code": code,
                "display_name": display_name,
            }),
        )),
        GatewayEvent::SessionCreated { session_id, agent_id, user_id } => Some((
            "session.created".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "agent_id": agent_id,
                "user_id": user_id,
            }),
        )),
        GatewayEvent::SessionRenamed { session_id, name } => Some((
            "session.renamed".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "name": name,
            }),
        )),
        GatewayEvent::AcpSpawned {
            session_id,
            subagent_id,
            parent_id,
            mode,
            thread_id,
        } => Some((
            "acp.spawned".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "subagent_id": subagent_id,
                "parent_id": parent_id,
                "mode": mode,
                "thread_id": thread_id,
            }),
        )),
        GatewayEvent::AcpCompleted {
            session_id,
            subagent_id,
            status,
        } => Some((
            "acp.completed".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "subagent_id": subagent_id,
                "status": status,
            }),
        )),
        GatewayEvent::AcpStatusChanged {
            session_id,
            runtime_state,
        } => Some((
            "acp.status_changed".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "runtime_state": runtime_state,
            }),
        )),
        GatewayEvent::AcpRecovered {
            session_id,
            old_subagent_id,
            new_subagent_id,
            crash_count,
        } => Some((
            "acp.recovered".to_string(),
            serde_json::json!({
                "session_id": session_id,
                "old_subagent_id": old_subagent_id,
                "new_subagent_id": new_subagent_id,
                "crash_count": crash_count,
            }),
        )),
        GatewayEvent::AcpThreadSwitched {
            thread_id,
            active_subagent,
        } => Some((
            "acp.thread_switched".to_string(),
            serde_json::json!({
                "thread_id": thread_id,
                "active_subagent": active_subagent,
            }),
        )),
    }
}

// ── Error Codes ───────────────────────────────────────────────────────────────

/// Build a standardized error response
pub fn error_unauthorized(id: impl Into<String>) -> WsResponse {
    WsResponse::err(id, "UNAUTHORIZED", "Invalid or missing authentication")
}

pub fn error_forbidden(id: impl Into<String>, missing: &str) -> WsResponse {
    WsResponse::err(id, "FORBIDDEN", format!("Missing required scope: {}", missing))
}

pub fn error_invalid_request(id: impl Into<String>, msg: impl Into<String>) -> WsResponse {
    WsResponse::err(id, "INVALID_REQUEST", msg)
}

pub fn error_method_not_found(id: impl Into<String>, method: &str) -> WsResponse {
    WsResponse::err(id, "METHOD_NOT_FOUND", format!("Unknown method: {}", method))
}

pub fn error_session_not_found(id: impl Into<String>) -> WsResponse {
    WsResponse::err(id, "SESSION_NOT_FOUND", "Session does not exist")
}

pub fn error_agent_not_found(id: impl Into<String>) -> WsResponse {
    WsResponse::err(id, "AGENT_NOT_FOUND", "Agent does not exist")
}

pub fn error_rate_limited(id: impl Into<String>) -> WsResponse {
    WsResponse::err(id, "RATE_LIMITED", "Too many requests")
}

pub fn error_internal(id: impl Into<String>, msg: impl Into<String>) -> WsResponse {
    WsResponse::err(id, "INTERNAL_ERROR", msg)
}

pub fn error_version_mismatch(id: impl Into<String>) -> WsResponse {
    WsResponse::err(id, "VERSION_MISMATCH", "Protocol version not supported")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_response_ok() {
        let res = WsResponse::ok("req_1", serde_json::json!({"status": "ok"}));
        assert!(res.ok);
        assert_eq!(res.id, "req_1");
        assert!(res.error.is_none());
    }

    #[test]
    fn test_ws_response_err() {
        let res = WsResponse::err("req_1", "TEST", "something failed");
        assert!(!res.ok);
        assert_eq!(res.id, "req_1");
        assert_eq!(res.error.as_ref().unwrap().code, "TEST");
    }

    #[test]
    fn test_scope_check() {
        let scopes = vec!["chat".to_string(), "read".to_string()];
        assert!(scopes_allow(&scopes, "chat.send"));
        assert!(scopes_allow(&scopes, "chat.history"));
        assert!(!scopes_allow(&scopes, "sessions.create"));

        let admin = vec!["admin".to_string()];
        assert!(scopes_allow(&admin, "anything.unknown"));
    }

    #[test]
    fn test_method_scope_mapping() {
        assert_eq!(method_scope("chat.send"), Some(SCOPE_CHAT));
        assert_eq!(method_scope("chat.history"), Some(SCOPE_READ));
        assert_eq!(method_scope("connect"), None);
        assert_eq!(method_scope("unknown"), Some(SCOPE_ADMIN));
    }

    #[test]
    fn test_protocol_connection() {
        let mut conn = ProtocolConnection::new("conn_1");
        assert!(!conn.handshaked);
        assert!(conn.is_subscribed("any")); // empty subscriptions = all

        conn.subscriptions.push("s1".to_string());
        assert!(conn.is_subscribed("s1"));
        assert!(!conn.is_subscribed("s2"));

        assert_eq!(conn.next_seq(), 1);
        assert_eq!(conn.next_seq(), 2);
    }

    #[test]
    fn test_acp_event_mapping() {
        let event = crate::gateway::GatewayEvent::AcpSpawned {
            session_id: "s1".to_string(),
            subagent_id: "sub-1".to_string(),
            parent_id: "parent".to_string(),
            mode: "run".to_string(),
            thread_id: "thread-1".to_string(),
        };
        let (name, payload) = gateway_event_to_ws(&event).expect("mapped event");
        assert_eq!(name, "acp.spawned");
        assert_eq!(payload["subagent_id"], "sub-1");

        let event = crate::gateway::GatewayEvent::AcpStatusChanged {
            session_id: "s1".to_string(),
            runtime_state: "paused".to_string(),
        };
        let (name, _) = gateway_event_to_ws(&event).expect("mapped event");
        assert_eq!(name, "acp.status_changed");
    }
}
