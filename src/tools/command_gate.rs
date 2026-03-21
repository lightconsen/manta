//! Command Gating for Manta
//!
//! Separates *chat* (conversational) interactions from *command* invocations
//! (e.g. `/skill install`, `/cron add`), allowing different permission levels
//! to be applied to each class of request.
//!
//! # Permission levels
//!
//! | Level   | Chat | Commands | Admin commands |
//! |---------|------|----------|----------------|
//! | `Chat`  | ✓    | ✗        | ✗              |
//! | `User`  | ✓    | ✓        | ✗              |
//! | `Admin` | ✓    | ✓        | ✓              |
//!
//! # Example
//!
//! ```rust
//! use manta::tools::command_gate::{CommandGate, UserLevel, AccessDecision};
//!
//! let gate = CommandGate::new();
//! gate.set_user_level("alice", UserLevel::User);
//! gate.set_user_level("bob", UserLevel::Admin);
//!
//! // A chat message is always allowed for registered users.
//! assert!(gate.check("alice", "/chat hello").is_allowed());
//!
//! // A slash command is only allowed at User level or above.
//! assert!(gate.check("alice", "/skill list").is_allowed());
//!
//! // An admin command requires Admin level.
//! assert!(!gate.check("alice", "/admin providers").is_allowed());
//! assert!(gate.check("bob", "/admin providers").is_allowed());
//! ```

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tracing::debug;

// ── Permission levels ─────────────────────────────────────────────────────────

/// The access level granted to a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UserLevel {
    /// Chat-only — can send messages but cannot invoke commands.
    Chat = 0,
    /// Standard user — can send messages and invoke user-level commands.
    User = 1,
    /// Administrator — full access including admin-only commands.
    Admin = 2,
}

impl Default for UserLevel {
    fn default() -> Self {
        UserLevel::Chat
    }
}

impl std::fmt::Display for UserLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UserLevel::Chat => write!(f, "chat"),
            UserLevel::User => write!(f, "user"),
            UserLevel::Admin => write!(f, "admin"),
        }
    }
}

// ── Command classification ────────────────────────────────────────────────────

/// The class of a request determined by its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestClass {
    /// Plain conversational message (no leading slash).
    Chat,
    /// A user-level slash command.
    Command,
    /// An admin-only slash command (prefixed with `/admin`).
    AdminCommand,
}

/// Admin command prefixes that require the `Admin` level.
const ADMIN_PREFIXES: &[&str] = &[
    "/admin",
    "/security audit",
    "/security pair",
    "/security revoke",
];

/// User command prefixes that require at least `User` level.
const USER_COMMAND_PREFIX: &str = "/";

impl RequestClass {
    /// Classify a message string.
    pub fn classify(content: &str) -> Self {
        let trimmed = content.trim();

        if ADMIN_PREFIXES.iter().any(|p| trimmed.starts_with(p)) {
            return RequestClass::AdminCommand;
        }

        if trimmed.starts_with(USER_COMMAND_PREFIX) {
            return RequestClass::Command;
        }

        RequestClass::Chat
    }

    /// The minimum `UserLevel` required to execute this class of request.
    pub fn required_level(&self) -> UserLevel {
        match self {
            RequestClass::Chat => UserLevel::Chat,
            RequestClass::Command => UserLevel::User,
            RequestClass::AdminCommand => UserLevel::Admin,
        }
    }
}

// ── Access decision ───────────────────────────────────────────────────────────

/// The result of a gate check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    /// The request is permitted.
    Allowed,
    /// The request is denied with a reason.
    Denied {
        /// Why access was denied.
        reason: String,
        /// The user's current level.
        current_level: UserLevel,
        /// The minimum level needed.
        required_level: UserLevel,
    },
}

impl AccessDecision {
    /// Return `true` if access is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, AccessDecision::Allowed)
    }
}

// ── Gate ──────────────────────────────────────────────────────────────────────

/// The command gate: maps user IDs to permission levels and evaluates requests.
///
/// Defaults to `UserLevel::Chat` for unknown users.
#[derive(Debug, Clone)]
pub struct CommandGate {
    levels: Arc<RwLock<HashMap<String, UserLevel>>>,
    unknown_user_level: UserLevel,
}

impl Default for CommandGate {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandGate {
    /// Create a new gate where unknown users get `Chat`-only access.
    pub fn new() -> Self {
        Self {
            levels: Arc::new(RwLock::new(HashMap::new())),
            unknown_user_level: UserLevel::Chat,
        }
    }

    /// Create a gate where unknown users are granted `User`-level access
    /// (open / trust-first policy).
    pub fn open() -> Self {
        Self {
            levels: Arc::new(RwLock::new(HashMap::new())),
            unknown_user_level: UserLevel::User,
        }
    }

    // ── Configuration ─────────────────────────────────────────────────────────

    /// Grant `level` to `user_id`.
    pub fn set_user_level(&self, user_id: impl Into<String>, level: UserLevel) {
        let mut map = self.levels.write().expect("CommandGate lock poisoned");
        map.insert(user_id.into(), level);
    }

    /// Remove a custom level for `user_id` (reverts to the default).
    pub fn clear_user_level(&self, user_id: &str) {
        let mut map = self.levels.write().expect("CommandGate lock poisoned");
        map.remove(user_id);
    }

    /// Retrieve the effective level for a user.
    pub fn user_level(&self, user_id: &str) -> UserLevel {
        let map = self.levels.read().expect("CommandGate lock poisoned");
        *map.get(user_id).unwrap_or(&self.unknown_user_level)
    }

    // ── Access checks ─────────────────────────────────────────────────────────

    /// Evaluate whether `user_id` may send `content`.
    pub fn check(&self, user_id: &str, content: &str) -> AccessDecision {
        let class = RequestClass::classify(content);
        let required = class.required_level();
        let current = self.user_level(user_id);

        if current >= required {
            debug!(
                user_id,
                ?class,
                level = %current,
                "CommandGate: access allowed"
            );
            AccessDecision::Allowed
        } else {
            debug!(
                user_id,
                ?class,
                current = %current,
                required = %required,
                "CommandGate: access denied"
            );
            AccessDecision::Denied {
                reason: format!(
                    "User '{}' has level '{}' but '{}' is required for this request",
                    user_id, current, required
                ),
                current_level: current,
                required_level: required,
            }
        }
    }

    /// Snapshot of all configured user levels.
    pub fn user_levels(&self) -> HashMap<String, UserLevel> {
        self.levels.read().expect("CommandGate lock poisoned").clone()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_chat() {
        assert_eq!(RequestClass::classify("hello there"), RequestClass::Chat);
        assert_eq!(RequestClass::classify("what is the weather?"), RequestClass::Chat);
    }

    #[test]
    fn test_classify_command() {
        assert_eq!(RequestClass::classify("/skill list"), RequestClass::Command);
        assert_eq!(RequestClass::classify("/cron add \"0 * * * *\" task"), RequestClass::Command);
    }

    #[test]
    fn test_classify_admin_command() {
        assert_eq!(RequestClass::classify("/admin providers"), RequestClass::AdminCommand);
        assert_eq!(RequestClass::classify("/security audit"), RequestClass::AdminCommand);
        assert_eq!(RequestClass::classify("/security pair --channel telegram"), RequestClass::AdminCommand);
    }

    #[test]
    fn test_chat_only_user() {
        let gate = CommandGate::new();
        gate.set_user_level("alice", UserLevel::Chat);

        assert!(gate.check("alice", "hello").is_allowed());
        assert!(!gate.check("alice", "/skill list").is_allowed());
        assert!(!gate.check("alice", "/admin providers").is_allowed());
    }

    #[test]
    fn test_standard_user() {
        let gate = CommandGate::new();
        gate.set_user_level("bob", UserLevel::User);

        assert!(gate.check("bob", "hello").is_allowed());
        assert!(gate.check("bob", "/skill list").is_allowed());
        assert!(!gate.check("bob", "/admin providers").is_allowed());
    }

    #[test]
    fn test_admin_user() {
        let gate = CommandGate::new();
        gate.set_user_level("carol", UserLevel::Admin);

        assert!(gate.check("carol", "hello").is_allowed());
        assert!(gate.check("carol", "/skill list").is_allowed());
        assert!(gate.check("carol", "/admin providers").is_allowed());
    }

    #[test]
    fn test_unknown_user_defaults_to_chat_level() {
        let gate = CommandGate::new();
        assert!(gate.check("unknown", "hello").is_allowed());
        assert!(!gate.check("unknown", "/skill list").is_allowed());
    }

    #[test]
    fn test_open_gate_allows_commands_for_unknown_users() {
        let gate = CommandGate::open();
        assert!(gate.check("stranger", "/skill list").is_allowed());
        assert!(!gate.check("stranger", "/admin providers").is_allowed());
    }

    #[test]
    fn test_clear_user_level_reverts_to_default() {
        let gate = CommandGate::new();
        gate.set_user_level("dave", UserLevel::Admin);
        gate.clear_user_level("dave");

        // Should revert to Chat-only.
        assert!(!gate.check("dave", "/skill list").is_allowed());
    }
}
