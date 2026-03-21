//! Send Policy Engine for Manta
//!
//! Evaluates allow/deny rules before the agent responds on any channel.
//! Rules are matched in priority order (highest first); the first matching
//! rule wins.  When no rule matches, the default policy applies.
//!
//! # Example
//!
//! ```rust
//! use manta::gateway::send_policy::{SendPolicy, PolicyRule, PolicyDecision, RuleCondition};
//!
//! let mut policy = SendPolicy::default();
//!
//! // Block a specific user
//! policy.add_rule(PolicyRule::deny("block-spammer")
//!     .condition(RuleCondition::UserId("spammer123".into()))
//!     .priority(100));
//!
//! let decision = policy.evaluate("spammer123", "telegram", "hello");
//! assert_eq!(decision, PolicyDecision::Deny { reason: "block-spammer".into() });
//! ```

use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use tracing::debug;

// ── Decision ──────────────────────────────────────────────────────────────────

/// The result of evaluating a `SendPolicy` for a given message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyDecision {
    /// Allow the message to be sent.
    Allow,
    /// Deny the message and return an optional reason to log.
    Deny {
        /// Human-readable reason for the denial.
        reason: String,
    },
    /// Allow but silently drop the reply (no error, no response).
    Silenced,
}

impl PolicyDecision {
    /// Return `true` if this decision permits the message.
    pub fn is_allow(&self) -> bool {
        matches!(self, PolicyDecision::Allow)
    }
}

// ── Conditions ────────────────────────────────────────────────────────────────

/// A condition that must match for a rule to apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleCondition {
    /// Match a specific user ID.
    UserId(String),
    /// Match a specific channel type (e.g. `"telegram"`, `"discord"`).
    Channel(String),
    /// Match when the message content contains the given substring.
    ContentContains(String),
    /// Match when the message content matches the given regex pattern.
    ContentMatches(String),
    /// Match all messages (wildcard).
    Any,
    /// Logical AND of sub-conditions.
    All(Vec<RuleCondition>),
    /// Logical OR of sub-conditions.
    AnyOf(Vec<RuleCondition>),
}

impl RuleCondition {
    /// Evaluate the condition against the given context.
    pub fn matches(&self, user_id: &str, channel: &str, content: &str) -> bool {
        match self {
            RuleCondition::UserId(id) => user_id == id,
            RuleCondition::Channel(ch) => channel == ch,
            RuleCondition::ContentContains(sub) => content.contains(sub.as_str()),
            RuleCondition::ContentMatches(pattern) => {
                // Use simple glob-style matching (avoid pulling in regex dep here)
                glob_match(pattern, content)
            }
            RuleCondition::Any => true,
            RuleCondition::All(conds) => {
                conds.iter().all(|c| c.matches(user_id, channel, content))
            }
            RuleCondition::AnyOf(conds) => {
                conds.iter().any(|c| c.matches(user_id, channel, content))
            }
        }
    }
}

// ── Rules ─────────────────────────────────────────────────────────────────────

/// A single policy rule mapping a condition to a decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Human-readable rule name used in log output and deny reasons.
    pub name: String,
    /// What to do when the condition matches.
    pub action: PolicyAction,
    /// The condition that triggers this rule.
    pub condition: RuleCondition,
    /// Higher-priority rules are evaluated first (descending order).
    pub priority: i32,
    /// Whether this rule is active.
    pub enabled: bool,
}

/// The action taken when a rule matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyAction {
    Allow,
    Deny,
    Silence,
}

impl PolicyRule {
    /// Create a new allow rule with the given name.
    pub fn allow(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            action: PolicyAction::Allow,
            condition: RuleCondition::Any,
            priority: 0,
            enabled: true,
        }
    }

    /// Create a new deny rule with the given name.
    pub fn deny(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            action: PolicyAction::Deny,
            condition: RuleCondition::Any,
            priority: 0,
            enabled: true,
        }
    }

    /// Create a new silence rule with the given name.
    pub fn silence(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            action: PolicyAction::Silence,
            condition: RuleCondition::Any,
            priority: 0,
            enabled: true,
        }
    }

    /// Set the condition for this rule.
    pub fn condition(mut self, condition: RuleCondition) -> Self {
        self.condition = condition;
        self
    }

    /// Set the priority for this rule.
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Disable this rule without removing it.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Check whether this rule applies to the given message context.
    pub fn applies(&self, user_id: &str, channel: &str, content: &str) -> bool {
        self.enabled && self.condition.matches(user_id, channel, content)
    }

    /// Convert the rule to a `PolicyDecision`.
    fn to_decision(&self) -> PolicyDecision {
        match self.action {
            PolicyAction::Allow => PolicyDecision::Allow,
            PolicyAction::Deny => PolicyDecision::Deny { reason: self.name.clone() },
            PolicyAction::Silence => PolicyDecision::Silenced,
        }
    }
}

// ── Engine ────────────────────────────────────────────────────────────────────

/// The send policy engine.
///
/// Thread-safe — clone the `Arc` to share across tasks.
#[derive(Debug, Clone)]
pub struct SendPolicy {
    inner: Arc<RwLock<PolicyInner>>,
}

#[derive(Debug, Default)]
struct PolicyInner {
    rules: Vec<PolicyRule>,
    default: DefaultPolicy,
}

/// What to do when no rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DefaultPolicy {
    /// Allow all unmatched messages (open by default). This is the default.
    #[default]
    Allow,
    /// Deny all unmatched messages (closed by default).
    Deny,
}

impl Default for SendPolicy {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(PolicyInner::default())),
        }
    }
}

impl SendPolicy {
    /// Create a new `SendPolicy` with the given default.
    pub fn new(default: DefaultPolicy) -> Self {
        Self {
            inner: Arc::new(RwLock::new(PolicyInner { rules: Vec::new(), default })),
        }
    }

    /// Add a rule to the policy.
    pub fn add_rule(&self, rule: PolicyRule) {
        let mut inner = self.inner.write().expect("SendPolicy lock poisoned");
        inner.rules.push(rule);
        // Sort descending by priority so we always evaluate high-priority first.
        inner.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
    }

    /// Remove all rules with the given name.
    pub fn remove_rule(&self, name: &str) {
        let mut inner = self.inner.write().expect("SendPolicy lock poisoned");
        inner.rules.retain(|r| r.name != name);
    }

    /// List all rules (for inspection/display).
    pub fn rules(&self) -> Vec<PolicyRule> {
        self.inner.read().expect("SendPolicy lock poisoned").rules.clone()
    }

    /// Set the default policy.
    pub fn set_default(&self, default: DefaultPolicy) {
        self.inner.write().expect("SendPolicy lock poisoned").default = default;
    }

    /// Evaluate the policy for a given message context.
    ///
    /// Returns the `PolicyDecision` for the first matching rule, or the
    /// default decision when no rule matches.
    pub fn evaluate(&self, user_id: &str, channel: &str, content: &str) -> PolicyDecision {
        let inner = self.inner.read().expect("SendPolicy lock poisoned");

        for rule in &inner.rules {
            if rule.applies(user_id, channel, content) {
                let decision = rule.to_decision();
                debug!(
                    rule = %rule.name,
                    user_id = %user_id,
                    channel = %channel,
                    ?decision,
                    "Send policy rule matched"
                );
                return decision;
            }
        }

        // No rule matched — apply the default.
        match inner.default {
            DefaultPolicy::Allow => PolicyDecision::Allow,
            DefaultPolicy::Deny => PolicyDecision::Deny { reason: "default-deny".into() },
        }
    }
}

// ── Glob matching ─────────────────────────────────────────────────────────────

/// Minimal glob matching: `*` matches any substring, `?` matches one char.
fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    glob_match_inner(&pat, &txt)
}

fn glob_match_inner(pat: &[char], txt: &[char]) -> bool {
    match (pat.first(), txt.first()) {
        (None, None) => true,
        (Some('*'), _) => {
            // Try consuming zero or more chars with the wildcard.
            glob_match_inner(&pat[1..], txt)
                || (!txt.is_empty() && glob_match_inner(pat, &txt[1..]))
        }
        (Some('?'), Some(_)) => glob_match_inner(&pat[1..], &txt[1..]),
        (Some(p), Some(t)) if p == t => glob_match_inner(&pat[1..], &txt[1..]),
        _ => false,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allow_by_default() {
        let policy = SendPolicy::default();
        assert_eq!(policy.evaluate("user1", "telegram", "hello"), PolicyDecision::Allow);
    }

    #[test]
    fn test_deny_specific_user() {
        let policy = SendPolicy::default();
        policy.add_rule(
            PolicyRule::deny("block-spammer")
                .condition(RuleCondition::UserId("spammer".into()))
                .priority(100),
        );

        assert_eq!(
            policy.evaluate("spammer", "telegram", "hello"),
            PolicyDecision::Deny { reason: "block-spammer".into() }
        );
        assert_eq!(policy.evaluate("legit-user", "telegram", "hello"), PolicyDecision::Allow);
    }

    #[test]
    fn test_silence_channel() {
        let policy = SendPolicy::default();
        policy.add_rule(
            PolicyRule::silence("mute-discord")
                .condition(RuleCondition::Channel("discord".into()))
                .priority(50),
        );

        assert_eq!(policy.evaluate("user", "discord", "hi"), PolicyDecision::Silenced);
        assert_eq!(policy.evaluate("user", "telegram", "hi"), PolicyDecision::Allow);
    }

    #[test]
    fn test_priority_ordering() {
        let policy = SendPolicy::default();
        // Lower-priority allow added first, higher-priority deny added second
        policy.add_rule(
            PolicyRule::allow("allow-all").condition(RuleCondition::Any).priority(1),
        );
        policy.add_rule(
            PolicyRule::deny("deny-user")
                .condition(RuleCondition::UserId("bad".into()))
                .priority(10),
        );

        // The deny rule has higher priority and should win.
        assert_eq!(
            policy.evaluate("bad", "any", "msg"),
            PolicyDecision::Deny { reason: "deny-user".into() }
        );
    }

    #[test]
    fn test_default_deny_policy() {
        let policy = SendPolicy::new(DefaultPolicy::Deny);
        assert_eq!(
            policy.evaluate("unknown", "telegram", "hello"),
            PolicyDecision::Deny { reason: "default-deny".into() }
        );

        // Explicit allow rule should override the default.
        policy.add_rule(
            PolicyRule::allow("allow-known").condition(RuleCondition::UserId("known".into())),
        );
        assert_eq!(policy.evaluate("known", "telegram", "hello"), PolicyDecision::Allow);
    }

    #[test]
    fn test_content_contains_condition() {
        let policy = SendPolicy::default();
        policy.add_rule(
            PolicyRule::deny("no-spam")
                .condition(RuleCondition::ContentContains("BUY NOW".into()))
                .priority(50),
        );

        assert_eq!(
            policy.evaluate("user", "telegram", "BUY NOW!!!"),
            PolicyDecision::Deny { reason: "no-spam".into() }
        );
        assert_eq!(policy.evaluate("user", "telegram", "normal message"), PolicyDecision::Allow);
    }

    #[test]
    fn test_all_condition() {
        let policy = SendPolicy::default();
        policy.add_rule(
            PolicyRule::deny("targeted-block")
                .condition(RuleCondition::All(vec![
                    RuleCondition::UserId("suspect".into()),
                    RuleCondition::Channel("telegram".into()),
                ]))
                .priority(100),
        );

        // Both conditions must match.
        assert_eq!(
            policy.evaluate("suspect", "telegram", "hi"),
            PolicyDecision::Deny { reason: "targeted-block".into() }
        );
        // Only one condition matches — rule should not fire.
        assert_eq!(policy.evaluate("suspect", "discord", "hi"), PolicyDecision::Allow);
    }

    #[test]
    fn test_glob_matching() {
        assert!(glob_match("hello*", "hello world"));
        assert!(glob_match("*world", "hello world"));
        assert!(glob_match("hel?o", "hello"));
        assert!(!glob_match("hel?o", "helloo"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("", ""));
    }

    #[test]
    fn test_remove_rule() {
        let policy = SendPolicy::default();
        policy.add_rule(
            PolicyRule::deny("temp-block")
                .condition(RuleCondition::UserId("user".into()))
                .priority(100),
        );

        assert_eq!(
            policy.evaluate("user", "any", "hi"),
            PolicyDecision::Deny { reason: "temp-block".into() }
        );

        policy.remove_rule("temp-block");
        assert_eq!(policy.evaluate("user", "any", "hi"), PolicyDecision::Allow);
    }
}
