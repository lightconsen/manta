//! Command gating for channel messages
//!
//! Restricts command execution to authorized users via access groups.
//! Supports:
//! - Named access groups with user membership
//! - Multi-authorizer OR logic (allow if ANY authorizer approves)
//! - Dual authorizer support (require TWO independent approvals)
//! - Channel-specific gate configuration
//! - Integration with existing PairingStore and DmPolicy

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Outcome of a command gate check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateResult {
    /// Command is allowed.
    Allowed,
    /// Command is denied.
    Denied(String),
}

impl GateResult {
    /// Returns true if the command is allowed.
    pub fn is_allowed(&self) -> bool {
        matches!(self, GateResult::Allowed)
    }
}

/// An access group that can be assigned to commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessGroup {
    /// Unique group name.
    pub name: String,
    /// Member user IDs.
    pub members: HashSet<String>,
    /// Group description.
    pub description: Option<String>,
}

impl AccessGroup {
    /// Create a new access group.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            members: HashSet::new(),
            description: None,
        }
    }

    /// Add a description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Add a member.
    pub fn add_member(&mut self, user_id: impl Into<String>) {
        self.members.insert(user_id.into());
    }

    /// Remove a member.
    pub fn remove_member(&mut self, user_id: &str) -> bool {
        self.members.remove(user_id)
    }

    /// Check if a user is a member of this group.
    pub fn contains(&self, user_id: &str) -> bool {
        self.members.contains(user_id)
    }
}

/// An authorizer that determines if a user is allowed to execute a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Authorizer {
    /// Allow if user is in a specific access group.
    GroupMember(String),
    /// Allow if user has been paired (via PairingStore).
    PairedUser,
    /// Allow if user is an admin.
    Admin,
    /// Allow if user is in the channel allowlist.
    Allowlisted,
    /// Allow if user matches a custom predicate (evaluated at runtime).
    Custom { name: String },
    /// Allow always (public command).
    Public,
    /// Allow never (disabled command).
    DenyAll,
}

impl Authorizer {
    /// Human-readable description.
    pub fn description(&self) -> &str {
        match self {
            Authorizer::GroupMember(_) => "group member",
            Authorizer::PairedUser => "paired user",
            Authorizer::Admin => "admin",
            Authorizer::Allowlisted => "allowlisted",
            Authorizer::Custom { name } => name.as_str(),
            Authorizer::Public => "public",
            Authorizer::DenyAll => "deny all",
        }
    }
}

/// Authorization logic for combining multiple authorizers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthorizerMode {
    /// Allow if ANY authorizer passes (OR logic).
    #[default]
    Any,
    /// Allow if ALL authorizers pass (AND logic).
    All,
}

/// Configuration for a command gate on a specific channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandGateConfig {
    /// Channel name (e.g., "telegram", "discord").
    pub channel: String,
    /// Authorizers for this channel.
    pub authorizers: Vec<Authorizer>,
    /// How to combine authorizers.
    pub mode: AuthorizerMode,
    /// Whether command gating is enabled for this channel.
    pub enabled: bool,
    /// Commands this gating applies to (empty = all commands).
    pub command_filter: Option<Vec<String>>,
}

impl CommandGateConfig {
    /// Create a new command gate config for a channel.
    pub fn new(channel: impl Into<String>) -> Self {
        Self {
            channel: channel.into(),
            authorizers: Vec::new(),
            mode: AuthorizerMode::Any,
            enabled: true,
            command_filter: None,
        }
    }

    /// Add an authorizer.
    pub fn with_authorizer(mut self, authorizer: Authorizer) -> Self {
        self.authorizers.push(authorizer);
        self
    }

    /// Set the authorizer mode.
    pub fn with_mode(mut self, mode: AuthorizerMode) -> Self {
        self.mode = mode;
        self
    }

    /// Disable this gate.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Apply a command filter.
    pub fn with_command_filter(mut self, commands: Vec<String>) -> Self {
        self.command_filter = Some(commands);
        self
    }

    /// Check if this gate applies to a given command.
    pub fn applies_to(&self, command: &str) -> bool {
        if let Some(ref filter) = self.command_filter {
            filter.iter().any(|c| c == command)
        } else {
            true // no filter = applies to all commands
        }
    }
}

/// Auth context passed during gate evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    /// The user's ID on the channel.
    pub user_id: String,
    /// The channel name.
    pub channel: String,
    /// The command being executed (without prefix).
    pub command: String,
    /// Whether the user is paired.
    pub is_paired: bool,
    /// Whether the user is an admin.
    pub is_admin: bool,
    /// Whether the user is allowlisted.
    pub is_allowlisted: bool,
    /// Whether the user is the verified owner.
    pub is_owner: bool,
    /// Optional provider/model hint for command execution.
    pub provider_hint: Option<String>,
    /// Custom auth flags set by the caller.
    pub custom_flags: HashMap<String, bool>,
}

impl AuthContext {
    /// Create a new auth context.
    pub fn new(
        user_id: impl Into<String>,
        channel: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            channel: channel.into(),
            command: command.into(),
            is_paired: false,
            is_admin: false,
            is_allowlisted: false,
            is_owner: false,
            provider_hint: None,
            custom_flags: HashMap::new(),
        }
    }

    /// Set paired status.
    pub fn with_paired(mut self, paired: bool) -> Self {
        self.is_paired = paired;
        self
    }

    /// Set admin status.
    pub fn with_admin(mut self, admin: bool) -> Self {
        self.is_admin = admin;
        self
    }

    /// Set allowlisted status.
    pub fn with_allowlisted(mut self, allowlisted: bool) -> Self {
        self.is_allowlisted = allowlisted;
        self
    }

    /// Set owner status.
    pub fn with_owner(mut self, owner: bool) -> Self {
        self.is_owner = owner;
        self
    }

    /// Set provider/model hint.
    pub fn with_provider_hint(mut self, hint: impl Into<String>) -> Self {
        self.provider_hint = Some(hint.into());
        self
    }

    /// Add a custom auth flag.
    pub fn with_flag(mut self, key: impl Into<String>, value: bool) -> Self {
        self.custom_flags.insert(key.into(), value);
        self
    }

    /// Build an auth context from an incoming message and channel policy.
    ///
    /// Populates `is_allowlisted` from the channel's `allow_from` list and
    /// `is_paired` from the optional pairing store.
    pub async fn from_message(msg: &super::IncomingMessage, policy: &super::ChannelPolicy) -> Self {
        let user_id = msg.user_id.0.clone();
        let channel = match &msg.provenance {
            super::InputProvenance::ExternalUser { channel, .. } => channel.clone(),
            _ => "unknown".to_string(),
        };

        let command = msg
            .metadata
            .extra
            .get("detected_command")
            .and_then(|v| v.get("command"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| msg.content.clone());

        let allow_list = policy.allow_from.read().await;
        let is_allowlisted = allow_list.iter().any(|a| a == &user_id);

        let is_paired = if let Some(store) = policy.pairing_store.read().await.as_ref() {
            store.is_authorized(&channel, &user_id).await
        } else {
            false
        };

        Self {
            user_id,
            channel,
            command,
            is_paired,
            is_admin: false,
            is_allowlisted,
            is_owner: false,
            provider_hint: None,
            custom_flags: HashMap::new(),
        }
    }
}

/// The command gate that controls access to commands on channels.
#[derive(Debug, Clone)]
pub struct CommandGate {
    /// Access groups (named user sets).
    groups: Arc<RwLock<HashMap<String, AccessGroup>>>,
    /// Per-channel gate configurations.
    configs: Arc<RwLock<HashMap<String, CommandGateConfig>>>,
}

impl CommandGate {
    /// Create a new empty command gate.
    pub fn new() -> Self {
        Self {
            groups: Arc::new(RwLock::new(HashMap::new())),
            configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a command gate with default admin-only config for all channels.
    pub fn with_admin_default() -> Self {
        let mut groups = HashMap::new();
        groups.insert(
            "admin".to_string(),
            AccessGroup::new("admin").with_description("Administrators"),
        );
        Self {
            groups: Arc::new(RwLock::new(groups)),
            configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    // ── Group management ──────────────────────────────────────────────

    /// Add or update an access group.
    pub async fn add_group(&self, group: AccessGroup) {
        let mut groups = self.groups.write().await;
        groups.insert(group.name.clone(), group);
    }

    /// Remove an access group.
    pub async fn remove_group(&self, name: &str) -> bool {
        let mut groups = self.groups.write().await;
        groups.remove(name).is_some()
    }

    /// Get a group by name.
    pub async fn get_group(&self, name: &str) -> Option<AccessGroup> {
        let groups = self.groups.read().await;
        groups.get(name).cloned()
    }

    /// Add a user to a group.
    pub async fn add_user_to_group(&self, group_name: &str, user_id: &str) -> bool {
        let mut groups = self.groups.write().await;
        if let Some(group) = groups.get_mut(group_name) {
            group.add_member(user_id);
            true
        } else {
            false
        }
    }

    /// Remove a user from a group.
    pub async fn remove_user_from_group(&self, group_name: &str, user_id: &str) -> bool {
        let mut groups = self.groups.write().await;
        groups
            .get_mut(group_name)
            .map(|g| g.remove_member(user_id))
            .unwrap_or(false)
    }

    /// Check if a user is in a group.
    pub async fn is_user_in_group(&self, group_name: &str, user_id: &str) -> bool {
        let groups = self.groups.read().await;
        groups
            .get(group_name)
            .map(|g| g.contains(user_id))
            .unwrap_or(false)
    }

    /// List all group names.
    pub async fn list_groups(&self) -> Vec<String> {
        let groups = self.groups.read().await;
        groups.keys().cloned().collect()
    }

    // ── Gate configuration ────────────────────────────────────────────

    /// Set the gate configuration for a channel.
    pub async fn set_config(&self, config: CommandGateConfig) {
        let channel = config.channel.clone();
        let mut configs = self.configs.write().await;
        configs.insert(channel, config);
    }

    /// Get the gate configuration for a channel.
    pub async fn get_config(&self, channel: &str) -> Option<CommandGateConfig> {
        let configs = self.configs.read().await;
        configs.get(channel).cloned()
    }

    /// Remove gate configuration for a channel (disables gating).
    pub async fn remove_config(&self, channel: &str) {
        let mut configs = self.configs.write().await;
        configs.remove(channel);
    }

    // ── Authorization ─────────────────────────────────────────────────

    /// Check if a user is authorized for a command on a channel.
    pub async fn check(&self, ctx: &AuthContext) -> GateResult {
        let configs = self.configs.read().await;
        let config = match configs.get(&ctx.channel) {
            Some(cfg) => cfg,
            None => {
                // No config = no gating = allowed
                return GateResult::Allowed;
            }
        };

        if !config.enabled {
            return GateResult::Allowed;
        }

        if !config.applies_to(&ctx.command) {
            return GateResult::Allowed;
        }

        if config.authorizers.is_empty() {
            return GateResult::Allowed;
        }

        // Evaluate each authorizer
        let mut results = Vec::new();
        for authorizer in &config.authorizers {
            let result = self.evaluate_authorizer(authorizer, ctx).await;
            if matches!(config.mode, AuthorizerMode::Any) && result {
                // OR mode: early exit on first pass
                return GateResult::Allowed;
            }
            results.push(result);
        }

        match config.mode {
            AuthorizerMode::Any => GateResult::Denied(format!(
                "Not authorized for command '{}' on {}. No authorizer approved.",
                ctx.command, ctx.channel
            )),
            AuthorizerMode::All => {
                let approved_count = results.iter().filter(|r| **r).count();
                let total = results.len();
                if approved_count == total {
                    GateResult::Allowed
                } else {
                    GateResult::Denied(format!(
                        "Not authorized for command '{}' on {}. Only {}/{} authorizers approved.",
                        ctx.command, ctx.channel, approved_count, total
                    ))
                }
            }
        }
    }

    /// Evaluate a single authorizer against the auth context.
    async fn evaluate_authorizer(&self, authorizer: &Authorizer, ctx: &AuthContext) -> bool {
        match authorizer {
            Authorizer::GroupMember(group_name) => {
                self.is_user_in_group(group_name, &ctx.user_id).await
            }
            Authorizer::PairedUser => ctx.is_paired,
            Authorizer::Admin => ctx.is_admin,
            Authorizer::Allowlisted => ctx.is_allowlisted,
            Authorizer::Public => true,
            Authorizer::DenyAll => false,
            Authorizer::Custom { name } => *ctx.custom_flags.get(name).unwrap_or(&false),
        }
    }

    /// Convenience: dual authorizer check (requires two specific authorizers).
    ///
    /// This is equivalent to setting mode=All with two authorizers.
    pub async fn check_dual(
        &self,
        ctx: &AuthContext,
        auth1: &Authorizer,
        auth2: &Authorizer,
    ) -> GateResult {
        let r1 = self.evaluate_authorizer(auth1, ctx).await;
        if !r1 {
            return GateResult::Denied(format!(
                "First authorizer ({}) denied for command '{}'",
                auth1.description(),
                ctx.command
            ));
        }

        let r2 = self.evaluate_authorizer(auth2, ctx).await;
        if !r2 {
            return GateResult::Denied(format!(
                "Second authorizer ({}) denied for command '{}'",
                auth2.description(),
                ctx.command
            ));
        }

        GateResult::Allowed
    }
}

impl Default for CommandGate {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse command and arguments from a message string.
///
/// Expected format: `/command arg1 arg2` or `!command arg1 arg2`
pub fn parse_command(content: &str, prefixes: &[&str]) -> Option<(String, Vec<String>)> {
    let trimmed = content.trim();
    for prefix in prefixes {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.is_empty() {
                return None;
            }
            let command = parts[0].to_lowercase();
            let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            return Some((command, args));
        }
    }
    None
}

/// Built-in access groups.
pub mod builtin_groups {
    use super::AccessGroup;

    /// Admin group — typically populated from config.
    pub fn admin() -> AccessGroup {
        AccessGroup::new("admin").with_description("Administrators")
    }

    /// Moderator group.
    pub fn moderator() -> AccessGroup {
        AccessGroup::new("moderator").with_description("Moderators")
    }

    /// Power user group.
    pub fn power_user() -> AccessGroup {
        AccessGroup::new("power_user").with_description("Power users")
    }

    /// Trusted user group.
    pub fn trusted() -> AccessGroup {
        AccessGroup::new("trusted").with_description("Trusted users")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_access_group() {
        let mut group = AccessGroup::new("admin").with_description("Admins");
        group.add_member("user1");
        group.add_member("user2");

        assert!(group.contains("user1"));
        assert!(group.contains("user2"));
        assert!(!group.contains("user3"));

        assert!(group.remove_member("user1"));
        assert!(!group.contains("user1"));
    }

    #[tokio::test]
    async fn test_gate_no_config_allows_all() {
        let gate = CommandGate::new();
        let ctx = AuthContext::new("user1", "telegram", "/help");
        assert_eq!(gate.check(&ctx).await, GateResult::Allowed);
    }

    #[tokio::test]
    async fn test_gate_disabled_allows_all() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram")
            .with_authorizer(Authorizer::DenyAll)
            .disabled();
        gate.set_config(config).await;

        let ctx = AuthContext::new("user1", "telegram", "/help");
        assert_eq!(gate.check(&ctx).await, GateResult::Allowed);
    }

    #[tokio::test]
    async fn test_gate_deny_all() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram").with_authorizer(Authorizer::DenyAll);
        gate.set_config(config).await;

        let ctx = AuthContext::new("user1", "telegram", "/help");
        assert!(matches!(gate.check(&ctx).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_gate_public_allows_all() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram").with_authorizer(Authorizer::Public);
        gate.set_config(config).await;

        let ctx = AuthContext::new("user1", "telegram", "/help");
        assert_eq!(gate.check(&ctx).await, GateResult::Allowed);
    }

    #[tokio::test]
    async fn test_gate_group_member() {
        let gate = CommandGate::new();
        gate.add_group(AccessGroup::new("beta").with_description("Beta testers"))
            .await;
        gate.add_user_to_group("beta", "user1").await;

        let config = CommandGateConfig::new("telegram")
            .with_authorizer(Authorizer::GroupMember("beta".to_string()));
        gate.set_config(config).await;

        // user1 is in beta group
        let ctx1 = AuthContext::new("user1", "telegram", "/beta_cmd");
        assert_eq!(gate.check(&ctx1).await, GateResult::Allowed);

        // user2 is not in beta group
        let ctx2 = AuthContext::new("user2", "telegram", "/beta_cmd");
        assert!(matches!(gate.check(&ctx2).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_gate_admin() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("discord").with_authorizer(Authorizer::Admin);
        gate.set_config(config).await;

        let ctx_admin = AuthContext::new("admin_user", "discord", "/admin_cmd").with_admin(true);
        assert_eq!(gate.check(&ctx_admin).await, GateResult::Allowed);

        let ctx_user = AuthContext::new("regular_user", "discord", "/admin_cmd").with_admin(false);
        assert!(matches!(gate.check(&ctx_user).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_gate_or_mode() {
        let gate = CommandGate::new();
        gate.add_group(AccessGroup::new("vip")).await;
        gate.add_user_to_group("vip", "vip_user").await;

        let config = CommandGateConfig::new("telegram")
            .with_authorizer(Authorizer::Admin)
            .with_authorizer(Authorizer::GroupMember("vip".to_string()))
            .with_mode(AuthorizerMode::Any); // OR logic
        gate.set_config(config).await;

        // VIP user (not admin) should be allowed
        let ctx_vip = AuthContext::new("vip_user", "telegram", "/cmd").with_admin(false);
        assert_eq!(gate.check(&ctx_vip).await, GateResult::Allowed);

        // Regular user (neither admin nor vip) should be denied
        let ctx_user = AuthContext::new("user", "telegram", "/cmd").with_admin(false);
        assert!(matches!(gate.check(&ctx_user).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_gate_and_mode_dual_authorizer() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram")
            .with_authorizer(Authorizer::Admin)
            .with_authorizer(Authorizer::PairedUser)
            .with_mode(AuthorizerMode::All); // AND logic
        gate.set_config(config).await;

        // Both conditions met
        let ctx_both = AuthContext::new("admin_user", "telegram", "/sensitive")
            .with_admin(true)
            .with_paired(true);
        assert_eq!(gate.check(&ctx_both).await, GateResult::Allowed);

        // Admin but not paired
        let ctx_admin_only = AuthContext::new("admin_user", "telegram", "/sensitive")
            .with_admin(true)
            .with_paired(false);
        assert!(matches!(gate.check(&ctx_admin_only).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_command_filter() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram")
            .with_authorizer(Authorizer::Admin)
            .with_command_filter(vec!["admin".to_string()]);
        gate.set_config(config).await;

        // Admin command — gate applies, user is not admin → denied
        let ctx_user = AuthContext::new("user", "telegram", "admin").with_admin(false);
        assert!(matches!(gate.check(&ctx_user).await, GateResult::Denied(_)));

        let ctx_user_help = AuthContext::new("user", "telegram", "help").with_admin(false);
        // No filter match = allowed
        assert_eq!(gate.check(&ctx_user_help).await, GateResult::Allowed);
    }

    #[tokio::test]
    async fn test_custom_authorizer() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram").with_authorizer(Authorizer::Custom {
            name: "feature_flag_x".to_string(),
        });
        gate.set_config(config).await;

        let ctx_enabled =
            AuthContext::new("user1", "telegram", "/cmd").with_flag("feature_flag_x", true);
        assert_eq!(gate.check(&ctx_enabled).await, GateResult::Allowed);

        let ctx_disabled = AuthContext::new("user1", "telegram", "/cmd");
        assert!(matches!(gate.check(&ctx_disabled).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_check_dual() {
        let gate = CommandGate::new();
        let ctx = AuthContext::new("user1", "telegram", "/dual_cmd")
            .with_admin(true)
            .with_paired(true);

        assert_eq!(
            gate.check_dual(&ctx, &Authorizer::Admin, &Authorizer::PairedUser)
                .await,
            GateResult::Allowed
        );

        let ctx_unpaired = AuthContext::new("user1", "telegram", "/dual_cmd")
            .with_admin(true)
            .with_paired(false);

        assert!(matches!(
            gate.check_dual(&ctx_unpaired, &Authorizer::Admin, &Authorizer::PairedUser)
                .await,
            GateResult::Denied(_)
        ));
    }

    #[test]
    fn test_parse_command() {
        let prefixes = &["/", "!"];

        let (cmd, args) = parse_command("/help", prefixes).unwrap();
        assert_eq!(cmd, "help");
        assert!(args.is_empty());

        let (cmd, args) = parse_command("/kick user1 reason", prefixes).unwrap();
        assert_eq!(cmd, "kick");
        assert_eq!(args, vec!["user1", "reason"]);

        let (cmd, args) = parse_command("!deploy --env prod", prefixes).unwrap();
        assert_eq!(cmd, "deploy");
        assert_eq!(args, vec!["--env", "prod"]);

        assert!(parse_command("not a command", prefixes).is_none());
        assert!(parse_command("", prefixes).is_none());
    }

    #[test]
    fn test_authorizer_description() {
        assert_eq!(Authorizer::Admin.description(), "admin");
        assert_eq!(Authorizer::Public.description(), "public");
        assert_eq!(Authorizer::DenyAll.description(), "deny all");
        assert_eq!(Authorizer::Custom { name: "flag".to_string() }.description(), "flag");
    }

    #[tokio::test]
    async fn test_group_management() {
        let gate = CommandGate::new();
        gate.add_group(AccessGroup::new("mods")).await;
        gate.add_user_to_group("mods", "mod1").await;
        gate.add_user_to_group("mods", "mod2").await;

        assert!(gate.is_user_in_group("mods", "mod1").await);
        assert!(gate.is_user_in_group("mods", "mod2").await);

        gate.remove_user_from_group("mods", "mod1").await;
        assert!(!gate.is_user_in_group("mods", "mod1").await);

        let groups = gate.list_groups().await;
        assert!(groups.contains(&"mods".to_string()));
    }

    #[tokio::test]
    async fn test_paired_user_authorizer() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram").with_authorizer(Authorizer::PairedUser);
        gate.set_config(config).await;

        let ctx_paired = AuthContext::new("user1", "telegram", "/cmd").with_paired(true);
        assert_eq!(gate.check(&ctx_paired).await, GateResult::Allowed);

        let ctx_unpaired = AuthContext::new("user2", "telegram", "/cmd").with_paired(false);
        assert!(matches!(gate.check(&ctx_unpaired).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_allowlisted_authorizer() {
        let gate = CommandGate::new();
        let config = CommandGateConfig::new("telegram").with_authorizer(Authorizer::Allowlisted);
        gate.set_config(config).await;

        let ctx_allowed = AuthContext::new("user1", "telegram", "/cmd").with_allowlisted(true);
        assert_eq!(gate.check(&ctx_allowed).await, GateResult::Allowed);

        let ctx_not_allowed = AuthContext::new("user2", "telegram", "/cmd").with_allowlisted(false);
        assert!(matches!(gate.check(&ctx_not_allowed).await, GateResult::Denied(_)));
    }

    #[tokio::test]
    async fn test_auth_context_from_message_allowlist() {
        let policy = crate::channels::ChannelPolicy::new(
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            std::sync::Arc::new(tokio::sync::RwLock::new(crate::security::pairing::DmPolicy::Open)),
            std::sync::Arc::new(tokio::sync::RwLock::new(vec!["alice".to_string()])),
        );

        let msg = crate::channels::IncomingMessage::new("alice", "conv1", "/help").with_provenance(
            crate::channels::InputProvenance::ExternalUser {
                channel: "telegram".to_string(),
                is_direct: true,
            },
        );

        let ctx = AuthContext::from_message(&msg, &policy).await;
        assert_eq!(ctx.user_id, "alice");
        assert_eq!(ctx.channel, "telegram");
        assert_eq!(ctx.command, "/help");
        assert!(ctx.is_allowlisted);
        assert!(!ctx.is_paired);

        let msg_bob = crate::channels::IncomingMessage::new("bob", "conv1", "/help")
            .with_provenance(crate::channels::InputProvenance::ExternalUser {
                channel: "telegram".to_string(),
                is_direct: true,
            });
        let ctx_bob = AuthContext::from_message(&msg_bob, &policy).await;
        assert!(!ctx_bob.is_allowlisted);
    }

    #[tokio::test]
    async fn test_auth_context_from_message_detected_command() {
        let policy = crate::channels::ChannelPolicy::new(
            std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            std::sync::Arc::new(tokio::sync::RwLock::new(crate::security::pairing::DmPolicy::Open)),
            std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        );

        let result = crate::tools::command_detector::detect_command("/skill list extra").unwrap();
        let metadata = crate::channels::MessageMetadata::new().with_detected_command(&result);
        let msg = crate::channels::IncomingMessage::new("alice", "conv1", "/skill list extra")
            .with_metadata(metadata);

        let ctx = AuthContext::from_message(&msg, &policy).await;
        assert_eq!(ctx.command, "skill");
    }
}
