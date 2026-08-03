//! [`ToolRegistry`]: tool storage, execution, caching, and policy gating.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tracing::{info, warn};

use super::util::consume_stream;
use super::{
    ApprovalDecision, ApprovalQueue, BoxedTool, PendingApproval, PolicyEvaluationContext,
    SharedTool, SkillTrust, Tool, ToolContext, ToolExecutionChunk, ToolExecutionResult, ToolHooks,
    ToolPolicyDecision,
};
use crate::providers::{FunctionCall, FunctionDefinition};

#[derive(Debug, Clone)]
struct CacheEntry {
    result: ToolExecutionResult,
    timestamp: std::time::Instant,
}

/// Shared, mutable list of web search providers. Held by both WebSearchTool
/// and ToolRegistry so hot-reload can update providers without rebuilding
/// the registry.
pub type WebSearchProviders =
    std::sync::Arc<tokio::sync::RwLock<Vec<crate::tools::web::SearchProvider>>>;

/// Registry of tools with optional caching, circuit breaker, and trust-level
/// filtering.
pub struct ToolRegistry {
    tools: std::sync::RwLock<HashMap<String, SharedTool>>,
    /// Dynamically registered tools (e.g. MCP auto-discovered tools).
    /// Uses interior mutability so tools can be added through
    /// `Arc<ToolRegistry>`.
    dynamic_tools: std::sync::RwLock<HashMap<String, std::sync::Arc<dyn Tool>>>,
    /// Tool-name prefixes that have been logically deregistered (e.g. MCP
    /// server disconnect). Tools matching any blocked prefix are excluded
    /// from `get`, `list`, `has`, `get_definitions`, and `get_available`
    /// without requiring `&mut self` — allowing this to be called through an
    /// `Arc<ToolRegistry>`.
    blocked_prefixes: std::sync::RwLock<HashSet<String>>,
    cache: std::sync::Mutex<HashMap<String, CacheEntry>>,
    cache_ttl: Option<Duration>,
    cache_enabled: bool,
    /// Per-tool failure counts for circuit breaker logic.
    failure_counts: std::sync::RwLock<HashMap<String, u32>>,
    /// Tool names that require `SkillTrust::Trusted` access.
    /// When a context has `skill_trust == Community` these tools are hidden.
    privileged_tools: std::sync::RwLock<HashSet<String>>,
    /// Hooks for tool execution (before/after/policy).
    hooks: ToolHooks,
    /// Runtime-override hooks (set through `&self` via `set_hooks`).
    /// Allows tests to inject policy hooks through an `Arc<ToolRegistry>`.
    /// Takes precedence over `self.hooks` when `Some`.
    hooks_override: std::sync::Mutex<Option<ToolHooks>>,
    /// Approval queue for human-in-the-loop tool execution.
    /// When set, high-risk tool calls can be suspended pending human approval.
    approval_queue: Option<Arc<ApprovalQueue>>,
    /// Content filter for scanning tool outputs for PII and secrets.
    content_filter: Option<Arc<crate::security::content_filter::ContentFilter>>,
    /// Audit logger for recording tool invocations and security events.
    audit_log: Option<Arc<dyn crate::security::runtime_audit::AuditLogger>>,
    /// Shared provider list for the web_search tool. Hot-reload updates this
    /// directly when `[search]` configuration changes.
    web_search_providers: Option<WebSearchProviders>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            tools: std::sync::RwLock::new(HashMap::new()),
            dynamic_tools: std::sync::RwLock::new(HashMap::new()),
            blocked_prefixes: std::sync::RwLock::new(HashSet::new()),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: None,
            cache_enabled: true,
            failure_counts: std::sync::RwLock::new(HashMap::new()),
            privileged_tools: std::sync::RwLock::new(HashSet::new()),
            hooks: ToolHooks::new(),
            hooks_override: std::sync::Mutex::new(None),
            approval_queue: None,
            content_filter: None,
            audit_log: None,
            web_search_providers: None,
        }
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field(
                "tools",
                &self
                    .tools
                    .read()
                    .map(|m| m.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
            .field("hooks", &self.hooks)
            .field("approval_queue", &self.approval_queue.is_some())
            .finish()
    }
}

impl ToolRegistry {
    /// Number of consecutive failures before a tool is circuit-broken.
    pub const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;

    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            tools: std::sync::RwLock::new(HashMap::new()),
            dynamic_tools: std::sync::RwLock::new(HashMap::new()),
            blocked_prefixes: std::sync::RwLock::new(HashSet::new()),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: None,
            cache_enabled: false,
            failure_counts: std::sync::RwLock::new(HashMap::new()),
            privileged_tools: std::sync::RwLock::new(HashSet::new()),
            hooks: ToolHooks::new(),
            hooks_override: std::sync::Mutex::new(None),
            approval_queue: None,
            content_filter: None,
            audit_log: None,
            web_search_providers: None,
        }
    }

    /// Create a new registry with caching enabled
    pub fn with_cache(ttl: Duration) -> Self {
        Self {
            tools: std::sync::RwLock::new(HashMap::new()),
            dynamic_tools: std::sync::RwLock::new(HashMap::new()),
            blocked_prefixes: std::sync::RwLock::new(HashSet::new()),
            cache: std::sync::Mutex::new(HashMap::new()),
            cache_ttl: Some(ttl),
            cache_enabled: true,
            failure_counts: std::sync::RwLock::new(HashMap::new()),
            privileged_tools: std::sync::RwLock::new(HashSet::new()),
            hooks: ToolHooks::new(),
            hooks_override: std::sync::Mutex::new(None),
            approval_queue: None,
            content_filter: None,
            audit_log: None,
            web_search_providers: None,
        }
    }

    /// Attach the shared web_search provider list so hot-reload can update it
    /// without rebuilding the registry.
    pub fn with_web_search_providers(mut self, providers: WebSearchProviders) -> Self {
        self.web_search_providers = Some(providers);
        self
    }

    /// Get a clone of the shared web_search provider list, if one was set.
    pub fn web_search_providers(&self) -> Option<WebSearchProviders> {
        self.web_search_providers.clone()
    }

    // ── Circuit breaker ───────────────────────────────────────────────────────

    /// Record a failure for `name`. After `CIRCUIT_BREAKER_THRESHOLD`
    /// consecutive failures the tool is considered degraded and excluded from
    /// `get_available()`.
    pub fn record_failure(&self, name: &str) {
        if let Ok(mut counts) = self.failure_counts.write() {
            let entry = counts.entry(name.to_string()).or_insert(0);
            *entry += 1;
            if *entry >= Self::CIRCUIT_BREAKER_THRESHOLD {
                tracing::warn!(
                    tool = name,
                    failures = *entry,
                    "Tool circuit-breaker tripped — marking as degraded"
                );
            }
        }
    }

    /// Reset the failure count for `name` (e.g. after a successful execution).
    pub fn reset_failure(&self, name: &str) {
        if let Ok(mut counts) = self.failure_counts.write() {
            counts.remove(name);
        }
    }

    /// Returns `true` if the tool has been circuit-broken due to repeated
    /// failures.
    pub fn is_degraded(&self, name: &str) -> bool {
        self.failure_counts
            .read()
            .map(|counts| counts.get(name).copied().unwrap_or(0) >= Self::CIRCUIT_BREAKER_THRESHOLD)
            .unwrap_or(false)
    }

    /// List all currently-degraded tool names.
    pub fn degraded_tools(&self) -> Vec<String> {
        self.failure_counts
            .read()
            .map(|counts| {
                counts
                    .iter()
                    .filter(|(_, &v)| v >= Self::CIRCUIT_BREAKER_THRESHOLD)
                    .map(|(k, _)| k.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    // ── Privilege / trust-level filtering ────────────────────────────────────

    /// Mark `name` as a privileged tool (shell execution, file writes, etc.).
    /// Privileged tools are hidden when `context.skill_trust == Community`.
    pub fn mark_privileged(&mut self, name: &str) {
        if let Ok(mut set) = self.privileged_tools.write() {
            set.insert(name.to_string());
        }
    }

    /// Returns `true` if `name` is a privileged tool.
    pub fn is_privileged(&self, name: &str) -> bool {
        self.privileged_tools
            .read()
            .map(|set| set.contains(name))
            .unwrap_or(false)
    }

    /// Returns `true` if `name` matches any blocked prefix.
    fn is_blocked(&self, name: &str) -> bool {
        self.blocked_prefixes
            .read()
            .map(|set| set.iter().any(|p| name.starts_with(p.as_str())))
            .unwrap_or(false)
    }

    /// Returns `true` if the tool should be excluded from availability checks,
    /// considering blocked prefixes, circuit-breaker state, trust level,
    /// plugin allowlists, and any RBAC/gating policy attached to the context.
    fn is_excluded(&self, name: &str, context: &ToolContext) -> bool {
        if self.is_blocked(name) {
            return true;
        }
        if self.is_degraded(name) {
            return true;
        }
        if context.model.skill_trust < SkillTrust::Trusted && self.is_privileged(name) {
            return true;
        }

        // Determine registration provenance for source gating.
        let is_dynamic = self.is_dynamic_tool(name);
        let is_mcp = name.starts_with("mcp__");

        // Plugin allowlist at the context level (runtime restriction).
        if is_dynamic && Self::is_plugin_like_name(name) {
            if let Some(allowlist) = context.plugin_allowlist() {
                let allowed = allowlist
                    .iter()
                    .any(|prefix| name == prefix || name.starts_with(prefix));
                if !allowed {
                    return true;
                }
            }
        }

        // Sandbox policy: require sandboxed tools.
        if let Some(sandbox_policy) = context.sandbox_policy() {
            if sandbox_policy.require_sandboxed {
                let caps = self.tool_capabilities(name);
                if !caps.sandboxed {
                    return true;
                }
            }
        }

        if let (Some(user_ctx), Some(policy)) = (&context.user_context, &context.model.tool_policy)
        {
            let capabilities = self.tool_capabilities(name);
            let eval_ctx = PolicyEvaluationContext {
                model_name: context.model.model_name.clone(),
                provider_name: context.model.provider_name.clone(),
                sender_id: context.sender_id.clone(),
                sender_is_owner: context.sender_is_owner,
                plugin_allowlist: context.plugin_allowlist().map(|s| s.to_vec()),
                model_capabilities: context.model.model_capabilities.clone(),
                is_dynamic,
                is_mcp,
            };
            if !policy.evaluate_with_context(user_ctx, name, &capabilities, &eval_ctx) {
                return true;
            }
        }
        false
    }

    /// Helper to look up tool capabilities from either registry.
    fn tool_capabilities(&self, name: &str) -> crate::tools::sdk::ToolCapabilities {
        self.tools
            .read()
            .ok()
            .and_then(|map| map.get(name).map(|t| t.capabilities()))
            .or_else(|| {
                self.dynamic_tools
                    .read()
                    .ok()
                    .and_then(|map| map.get(name).map(|t| t.capabilities()))
            })
            .unwrap_or_default()
    }

    /// Get the advertised capabilities for a tool by name.
    pub fn get_capabilities(&self, name: &str) -> crate::tools::sdk::ToolCapabilities {
        self.tool_capabilities(name)
    }

    /// Returns `true` if `name` is registered only in the dynamic registry.
    fn is_dynamic_tool(&self, name: &str) -> bool {
        self.tools
            .read()
            .ok()
            .is_none_or(|map| !map.contains_key(name))
            && self
                .dynamic_tools
                .read()
                .map(|map| map.contains_key(name))
                .unwrap_or(false)
    }

    /// Heuristic: plugin tools often use `__` separators (MCP or plugin
    /// runtime).
    fn is_plugin_like_name(name: &str) -> bool {
        name.contains("__")
    }

    /// Enable caching with the specified TTL
    pub fn enable_cache(&mut self, ttl: Duration) {
        self.cache_enabled = true;
        self.cache_ttl = Some(ttl);
    }

    /// Disable caching
    pub fn disable_cache(&mut self) {
        self.cache_enabled = false;
        // Clear existing cache
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// Clear the tool result cache
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    // ── Unified tool iteration ───────────────────────────────────────────────

    /// Iterate over both static and dynamic registries, yielding
    /// `(name, Arc<dyn Tool>)` for every tool that satisfies `filter`.
    ///
    /// This is the single point of iteration for `list()`, `get_definitions()`,
    /// `get_available()`, and `all_tools_arc()` — they all delegate here rather
    /// than duplicating the two-registry walk.
    fn iter_tools<F>(&self, filter: F) -> Vec<(String, Arc<dyn Tool>)>
    where
        F: Fn(&str) -> bool,
    {
        let mut result = Vec::new();
        if let Ok(map) = self.tools.read() {
            for (name, tool) in map.iter() {
                if filter(name) {
                    result.push((name.clone(), tool.clone()));
                }
            }
        }
        if let Ok(dynamic) = self.dynamic_tools.read() {
            for (name, tool) in dynamic.iter() {
                if filter(name) {
                    result.push((name.clone(), tool.clone()));
                }
            }
        }
        result
    }

    /// Generate a cache key from tool name and arguments
    fn cache_key(name: &str, args: &Value) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        // Hash the JSON string representation of args
        args.to_string().hash(&mut hasher);
        format!("{}:{}", name, hasher.finish())
    }

    /// Get cached result if available and not expired
    fn get_cached(&self, key: &str) -> Option<ToolExecutionResult> {
        if !self.cache_enabled {
            return None;
        }

        let cache = match self.cache.lock() {
            Ok(guard) => guard,
            Err(e) => {
                warn!("Cache mutex poisoned in get_cached: {}", e);
                return None;
            }
        };
        let entry = cache.get(key)?;

        // Check if cache entry is expired
        if let Some(ttl) = self.cache_ttl {
            if entry.timestamp.elapsed() > ttl {
                return None;
            }
        }

        Some(entry.result.clone())
    }

    /// Store result in cache
    fn store_cached(&self, key: String, result: ToolExecutionResult) {
        if !self.cache_enabled {
            return;
        }

        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(
                key,
                CacheEntry {
                    result,
                    timestamp: std::time::Instant::now(),
                },
            );
        }
    }

    // ── Hooks and approval queue ──────────────────────────────────────────────

    /// Set the hooks for this registry.
    ///
    /// Hooks allow policy decisions, before/after execution callbacks,
    /// and human-in-the-loop approval for high-risk tools.
    pub fn with_hooks(mut self, hooks: ToolHooks) -> Self {
        self.hooks = hooks;
        self
    }

    /// Set the hooks for this registry through `&self` (interior mutability).
    ///
    /// This allows setting hooks through an `Arc<ToolRegistry>` without
    /// requiring `&mut self`. Used by tests that need to inject policy
    /// hooks at runtime (e.g. auto-approval for device tool calls).
    pub fn set_hooks(&self, hooks: ToolHooks) {
        if let Ok(mut guard) = self.hooks_override.lock() {
            *guard = Some(hooks);
        }
    }

    /// Return the active hooks — the override hooks if set, otherwise the
    /// builder-configured hooks.  Override hooks take precedence so that
    /// `set_hooks()` (called through `Arc<ToolRegistry>`) can inject hooks
    /// at runtime without requiring `&mut self`.
    fn active_hooks(&self) -> ToolHooks {
        self.hooks_override
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| self.hooks.clone())
    }

    /// Set the approval queue for human-in-the-loop execution.
    ///
    /// When set, tool calls that return `ToolPolicyDecision::NeedsApproval`
    /// will suspend execution and wait for human approval via the queue.
    pub fn with_approval_queue(mut self, queue: Arc<ApprovalQueue>) -> Self {
        self.approval_queue = Some(queue);
        self
    }

    /// Get a reference to the approval queue if set.
    pub fn approval_queue(&self) -> Option<&Arc<ApprovalQueue>> {
        self.approval_queue.as_ref()
    }

    /// Set the content filter for scanning tool outputs.
    pub fn with_content_filter(
        mut self,
        filter: Arc<crate::security::content_filter::ContentFilter>,
    ) -> Self {
        self.content_filter = Some(filter);
        self
    }

    /// Set the audit logger for recording security events.
    pub fn with_audit_log(
        mut self,
        audit_log: Arc<dyn crate::security::runtime_audit::AuditLogger>,
    ) -> Self {
        self.audit_log = Some(audit_log);
        self
    }

    /// Get a clone of the configured audit logger, if any.
    pub fn audit_log(&self) -> Option<Arc<dyn crate::security::runtime_audit::AuditLogger>> {
        self.audit_log.clone()
    }

    /// Get a clone of the configured content filter, if any.
    pub fn content_filter(&self) -> Option<Arc<crate::security::content_filter::ContentFilter>> {
        self.content_filter.clone()
    }

    /// Return a snapshot of all dynamically-registered tools.
    pub fn dynamic_tools(&self) -> Vec<(String, std::sync::Arc<dyn Tool>)> {
        match self.dynamic_tools.read() {
            Ok(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            Err(e) => {
                warn!("dynamic_tools lock poisoned: {}", e);
                Vec::new()
            }
        }
    }

    /// Register a tool from a boxed implementation.
    pub fn register(&mut self, tool: BoxedTool) {
        let name = tool.name().to_string();
        let tool: SharedTool = tool.into();
        match self.tools.write() {
            Ok(mut map) => {
                map.insert(name, tool);
            }
            Err(e) => warn!("Tools RwLock poisoned in register: {}", e),
        }
    }

    /// Remove a single tool by exact name.
    pub fn remove(&mut self, name: &str) -> Option<SharedTool> {
        match self.tools.write() {
            Ok(mut map) => map.remove(name),
            Err(e) => {
                warn!("Tools RwLock poisoned in remove: {}", e);
                None
            }
        }
    }

    /// Replace a statically-registered tool by exact name.
    /// Returns the previous tool if one existed.
    pub fn replace(&mut self, name: &str, tool: BoxedTool) -> Option<SharedTool> {
        let new_name = tool.name().to_string();
        if name != new_name {
            warn!("Tool replacement name mismatch: replacing '{}' with '{}'", name, new_name);
        }
        let tool: SharedTool = tool.into();
        match self.tools.write() {
            Ok(mut map) => map.insert(new_name, tool),
            Err(e) => {
                warn!("Tools RwLock poisoned in replace: {}", e);
                None
            }
        }
    }

    /// Remove all tools whose names start with `prefix`.
    ///
    /// Uses interior mutability so it works through `Arc<ToolRegistry>` —
    /// tools are hidden from all lookup methods immediately. The underlying
    /// map entries are lazily cleaned up (they remain allocated but invisible).
    ///
    /// Used by the MCP subsystem to clean up `mcp__{server}__*` tools when a
    /// server disconnects.
    pub fn deregister_prefix(&self, prefix: &str) {
        if let Ok(mut set) = self.blocked_prefixes.write() {
            set.insert(prefix.to_string());
        }
        // Also remove matching static and dynamic tools immediately so
        // stale entries don't accumulate in memory.
        if let Ok(mut map) = self.tools.write() {
            map.retain(|k, _| !k.starts_with(prefix));
        }
        if let Ok(mut map) = self.dynamic_tools.write() {
            map.retain(|k, _| !k.starts_with(prefix));
        }
    }

    /// Dynamically register a tool without requiring `&mut self`.
    ///
    /// This allows tools to be added through an `Arc<ToolRegistry>` — used by
    /// the MCP subsystem to register auto-discovered tools at startup.
    pub fn register_dynamic(&self, tool: std::sync::Arc<dyn Tool>) {
        let name = tool.name().to_string();
        if let Ok(mut map) = self.dynamic_tools.write() {
            map.insert(name, tool);
        }
    }

    /// Remove a single dynamically-registered tool by exact name.
    pub fn deregister_dynamic(&self, name: &str) {
        if let Ok(mut map) = self.dynamic_tools.write() {
            map.remove(name);
        }
    }

    /// Get a tool by name (returns `None` for blocked or degraded tools).
    ///
    /// Only covers statically-registered tools. For dynamic tools use
    /// `execute()` or `execute_call()` which check both registries.
    pub fn get(&self, name: &str) -> Option<SharedTool> {
        if self.is_blocked(name) || self.is_degraded(name) {
            return None;
        }
        self.tools
            .read()
            .ok()
            .and_then(|map| map.get(name).cloned())
    }

    /// List available tool names (excludes blocked and degraded tools).
    /// Includes both statically- and dynamically-registered tools.
    /// List available tool names (excludes blocked and degraded tools).
    /// Includes both statically- and dynamically-registered tools.
    pub fn list(&self) -> Vec<String> {
        self.iter_tools(|name| !self.is_blocked(name) && !self.is_degraded(name))
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Get all dynamically-registered tools as `Arc<dyn Tool>` references.
    ///
    /// Excludes blocked and degraded tools. Static tools registered via
    /// `register(Box<dyn Tool>)` are NOT returned — callers that need
    /// `Arc<dyn Tool>` for static tools should collect `Arc` references
    /// at registration time via `register_arc()`.
    pub fn all_tools_arc(&self) -> Vec<std::sync::Arc<dyn Tool>> {
        let mut result: Vec<std::sync::Arc<dyn Tool>> = Vec::new();

        if let Ok(dynamic) = self.dynamic_tools.read() {
            for (name, tool) in dynamic.iter() {
                if !self.is_blocked(name) && !self.is_degraded(name) {
                    result.push(tool.clone());
                }
            }
        }

        result
    }

    /// Check if a tool exists, is not blocked, and is not degraded.
    /// Checks both static and dynamic registries.
    pub fn has(&self, name: &str) -> bool {
        if self.is_blocked(name) || self.is_degraded(name) {
            return false;
        }
        if self
            .tools
            .read()
            .ok()
            .is_some_and(|map| map.contains_key(name))
        {
            return true;
        }
        self.dynamic_tools
            .read()
            .map(|map| map.contains_key(name))
            .unwrap_or(false)
    }

    /// Get all tools as function definitions (excludes blocked and degraded
    /// tools). Includes both statically- and dynamically-registered tools.
    pub fn get_definitions(&self) -> Vec<FunctionDefinition> {
        self.iter_tools(|name| !self.is_blocked(name) && !self.is_degraded(name))
            .into_iter()
            .map(|(_, tool)| tool.to_function_definition())
            .collect()
    }

    /// Get all available tools for a given context.
    ///
    /// Excludes:
    /// - Blocked-prefix tools (MCP server disconnected)
    /// - Degraded tools (circuit-breaker tripped)
    /// - Privileged tools when `context.skill_trust == Community`
    ///
    /// Includes both statically- and dynamically-registered tools.
    pub fn get_available(&self, context: &ToolContext) -> Vec<FunctionDefinition> {
        self.iter_tools(|name| !self.is_excluded(name, context))
            .into_iter()
            .filter(|(_, tool)| tool.is_available(context))
            .map(|(_, tool)| tool.to_function_definition())
            .collect()
    }

    /// Execute a tool by name with optional caching, hooks, and approval flow.
    /// Checks both static and dynamic registries.
    ///
    /// # Policy and Approval Flow
    ///
    /// Run policy hooks and the built-in `requires_approval` fallback.
    ///
    /// If no explicit policy hooks are configured but the tool advertises
    /// `requires_approval`, this synthesises a `NeedsApproval` decision
    /// automatically so that high-risk tools (device access, etc.) are
    /// never executed silently without the caller going through approval.
    async fn evaluate_policy(&self, name: &str, args: &Value) -> ToolPolicyDecision {
        let mut decision = self.active_hooks().run_policy(name, args).await;

        // requires_approval fallback — only when no policy hook exists, so
        // an explicitly-configured policy hook is always authoritative.
        if matches!(decision, ToolPolicyDecision::Allow) && !self.active_hooks().has_policy_hooks()
        {
            let caps = self.get_capabilities(name);
            if caps.requires_approval {
                let approval_id = format!(
                    "fallback-{}-{}",
                    name,
                    uuid::Uuid::new_v4()
                        .to_string()
                        .split('-')
                        .next()
                        .unwrap_or("0000")
                );
                decision = ToolPolicyDecision::NeedsApproval {
                    approval_id,
                    tool_name: name.to_string(),
                    args: args.clone(),
                    risk_level: crate::tools::approval::RiskLevel::High,
                    approval_level: crate::tools::approval::ApprovalLevel::Ask,
                    requested_by: "system".to_string(),
                    message: format!(
                        "Tool '{}' requires approval (fallsback from requires_approval flag)",
                        name
                    ),
                };
            }
        }

        decision
    }

    /// Execute a tool by name with optional caching, hooks, and approval flow.
    /// Checks both static and dynamic registries.
    ///
    /// # Policy and Approval Flow
    ///
    /// 1. Run policy hooks — if any hook returns `Deny`, return error
    ///    immediately
    /// 2. If any hook returns `NeedsApproval` and approval_queue is configured,
    ///    suspend execution and wait for human approval
    /// 3. Run before-hooks
    /// 4. Execute the tool
    /// 5. Run after-hooks
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        let policy_decision = self.evaluate_policy(name, &args).await;

        match policy_decision {
            ToolPolicyDecision::Allow => {
                // Proceed with execution
            }
            ToolPolicyDecision::Deny { reason } => {
                return Some(Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' denied: {}",
                    name, reason
                ))));
            }
            ToolPolicyDecision::NeedsApproval {
                approval_id,
                tool_name,
                args: approval_args,
                risk_level,
                approval_level,
                requested_by,
                message,
            } => {
                // Check if approval queue is configured
                let approval_queue = match &self.approval_queue {
                    Some(q) => q.clone(),
                    None => {
                        return Some(Err(crate::error::SyscityError::Validation(
                            "Tool requires approval but no approval queue configured".into(),
                        )));
                    }
                };

                // Create oneshot channel for the approval resolution
                let (tx, rx) = tokio::sync::oneshot::channel();

                // Create pending approval
                let approval = PendingApproval::new(
                    &approval_id,
                    &tool_name,
                    approval_args,
                    requested_by,
                    risk_level,
                    approval_level,
                    message,
                    tx,
                );

                // Submit to approval queue
                approval_queue.submit(approval).await;

                // Wait for human decision (with 5-minute timeout)
                const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);
                match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
                    Ok(Ok(ApprovalDecision::Approve)) => {
                        tracing::info!(
                            "Approval {} granted, proceeding with tool execution",
                            approval_id
                        );
                        // Proceed with execution below
                    }
                    Ok(Ok(ApprovalDecision::Deny { reason })) => {
                        return Some(Err(crate::error::SyscityError::Validation(format!(
                            "Tool '{}' denied by user: {}",
                            name, reason
                        ))));
                    }
                    Ok(Err(_)) => {
                        return Some(Err(crate::error::SyscityError::Validation(
                            "Approval channel closed".into(),
                        )));
                    }
                    Err(_) => {
                        return Some(Err(crate::error::SyscityError::Timeout(format!(
                            "Tool '{}' approval request timed out after {:?}",
                            name, APPROVAL_TIMEOUT
                        ))));
                    }
                }
            }
        }

        // Run before-hooks
        self.active_hooks().run_before(name, &args).await;

        // Check cache first
        let cache_key = Self::cache_key(name, &args);
        if let Some(cached_result) = self.get_cached(&cache_key) {
            tracing::debug!("Cache hit for tool: {}", name);
            let result = Ok(cached_result);
            if let Ok(ref exec_result) = result {
                self.active_hooks()
                    .run_after(name, &args, exec_result)
                    .await;
            }
            return self.filter_and_audit(name, context, Some(result)).await;
        }

        // Execute the tool — clone args so the original remains for after-hooks
        let execution_result: Option<crate::Result<ToolExecutionResult>> = {
            // Try static tools first
            if let Some(tool) = self.get(name) {
                let _t_exec = std::time::Instant::now();
                let result = tool.execute(args.clone(), context).await;
                info!("[Timing] tool.execute({}) returned in {:?}", name, _t_exec.elapsed());
                if let Ok(ref exec_result) = result {
                    self.store_cached(cache_key, exec_result.clone());
                }
                Some(result)
            } else {
                // Try dynamic tools
                let dynamic_tool = self
                    .dynamic_tools
                    .read()
                    .ok()
                    .and_then(|map| map.get(name).cloned());

                if let Some(tool) = dynamic_tool {
                    if !self.is_blocked(name) && !self.is_degraded(name) {
                        let result = tool.execute(args.clone(), context).await;
                        if let Ok(ref exec_result) = result {
                            self.store_cached(cache_key, exec_result.clone());
                        }
                        Some(result)
                    } else {
                        Some(Err(crate::error::SyscityError::Validation(format!(
                            "Tool '{}' is blocked or degraded",
                            name
                        ))))
                    }
                } else {
                    None
                }
            }
        };

        // Run after-hooks
        if let Some(Ok(ref exec_result)) = execution_result {
            let _t_hook = std::time::Instant::now();
            self.active_hooks()
                .run_after(name, &args, exec_result)
                .await;
            info!("[Timing] run_after({}) done in {:?}", name, _t_hook.elapsed());
        }

        let _t_filter = std::time::Instant::now();
        let result = self.filter_and_audit(name, context, execution_result).await;
        info!("[Timing] filter_and_audit({}) done in {:?}", name, _t_filter.elapsed());
        result
    }

    /// Apply content filtering and audit logging to a tool execution result.
    async fn filter_and_audit(
        &self,
        name: &str,
        context: &ToolContext,
        result: Option<crate::Result<ToolExecutionResult>>,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        // ── Audit: tool invocation ─────────────────────────────────────────
        if let Some(ref audit) = self.audit_log {
            let allowed = matches!(result, Some(Ok(_)));
            audit
                .log_entry(
                    crate::security::runtime_audit::AuditEventType::ToolInvocation,
                    context.user_id.clone(),
                    name.to_string(),
                    allowed,
                    format!("Tool '{}' executed", name),
                    None,
                )
                .await;
        }

        // ── Content filtering ──────────────────────────────────────────────
        let result = match result {
            Some(Ok(exec_result)) => {
                // Let the tool itself decide whether content filtering applies
                let skip_filter = self
                    .get(name)
                    .map(|t| t.skip_content_filter())
                    .unwrap_or(false);

                if skip_filter {
                    Some(Ok(exec_result))
                } else if let Some(ref filter) = self.content_filter {
                    let outcome = filter.filter_result(&exec_result);

                    // Audit: content filter action
                    if let Some(ref audit) = self.audit_log {
                        if outcome.action != crate::security::content_filter::FilterAction::Pass {
                            let details = serde_json::json!({
                                "action": format!("{:?}", outcome.action),
                                "pii_findings": outcome.pii_findings.len(),
                                "secret_findings": outcome.secret_findings.len(),
                                "summary": outcome.summary,
                            });
                            audit
                                .log_entry(
                                    crate::security::runtime_audit::AuditEventType::ContentFilter,
                                    context.user_id.clone(),
                                    name.to_string(),
                                    outcome.action
                                        != crate::security::content_filter::FilterAction::Blocked,
                                    outcome.summary.clone(),
                                    Some(details),
                                )
                                .await;
                        }
                    }

                    let filtered = crate::tools::ToolExecutionResult {
                        success: if outcome.action
                            == crate::security::content_filter::FilterAction::Blocked
                        {
                            false
                        } else {
                            outcome.success
                        },
                        output: outcome.output,
                        error: if outcome.action
                            == crate::security::content_filter::FilterAction::Blocked
                        {
                            Some(outcome.summary)
                        } else {
                            exec_result.error
                        },
                        data: outcome.data,
                        execution_time: exec_result.execution_time,
                    };
                    Some(Ok(filtered))
                } else {
                    Some(Ok(exec_result))
                }
            }
            other => other,
        };

        result
    }

    /// Execute a tool by name, skipping the cache layer but still running the
    /// full policy, approval, hooks, and audit pipeline.
    ///
    /// Returns `None` only when the tool name is unknown (not registered).
    /// Blocked, degraded, or policy-denied tools return `Some(Err(...))`
    /// so callers can distinguish "not found" from "rejected".
    ///
    /// This is `pub(crate)` for use by other modules in the crate
    /// (e.g. streaming execution paths) that need to bypass the cache
    /// without sacrificing safety checks, though currently no external
    /// caller exists.
    #[cfg(test)]
    pub(crate) async fn execute_no_cache(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        // Run policy evaluation (approval, denials, hooks all handled here).
        let policy_decision = self.evaluate_policy(name, &args).await;
        match policy_decision {
            ToolPolicyDecision::Allow => { /* proceed */ }
            ToolPolicyDecision::Deny { reason } => {
                return Some(Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' denied: {}",
                    name, reason
                ))));
            }
            ToolPolicyDecision::NeedsApproval { .. } => {
                // execute_no_cache does not support the full approval flow;
                // callers should use `execute()` instead.
                return Some(Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' requires approval; use execute() instead of execute_no_cache",
                    name,
                ))));
            }
        }

        // Run before-hooks
        self.active_hooks().run_before(name, &args).await;

        // Execute the tool
        let execution_result: Option<crate::Result<ToolExecutionResult>> = {
            // Try static tools first
            if let Some(tool) = self.get(name) {
                Some(tool.execute(args.clone(), context).await)
            } else {
                // Try dynamic tools
                let dynamic_tool = self
                    .dynamic_tools
                    .read()
                    .ok()
                    .and_then(|map| map.get(name).cloned());
                if let Some(tool) = dynamic_tool {
                    if !self.is_blocked(name) && !self.is_degraded(name) {
                        Some(tool.execute(args.clone(), context).await)
                    } else {
                        Some(Err(crate::error::SyscityError::Validation(format!(
                            "Tool '{}' is blocked or degraded",
                            name
                        ))))
                    }
                } else {
                    None
                }
            }
        };

        // Run after-hooks
        if let Some(Ok(ref exec_result)) = execution_result {
            let _t_hook = std::time::Instant::now();
            self.active_hooks()
                .run_after(name, &args, exec_result)
                .await;
            info!("[Timing] run_after({}) done in {:?}", name, _t_hook.elapsed());
        }

        let _t_filter = std::time::Instant::now();
        let result = self.filter_and_audit(name, context, execution_result).await;
        info!("[Timing] filter_and_audit({}) done in {:?}", name, _t_filter.elapsed());
        result
    }

    /// Parse tool call arguments, handling provider-specific edge cases.
    ///
    /// Some providers (DeepSeek) append trailing text after the JSON object
    /// or emit multiple JSON values. This uses a streaming parser that
    /// extracts only the first valid JSON value and ignores trailing content.
    pub(super) fn parse_tool_args(&self, raw: &str, tool_name: &str) -> crate::Result<Value> {
        let s = raw.trim();
        if s.is_empty() {
            return Ok(serde_json::json!({}));
        }

        // Fast path: direct parse works for clean JSON.
        if let Ok(val) = serde_json::from_str::<Value>(s) {
            return Ok(val);
        }

        // Fallback: streaming parser extracts only the first JSON value,
        // ignoring any trailing text or multiple objects.
        let stream = serde_json::Deserializer::from_str(s);
        if let Some(value) = stream.into_iter::<Value>().next() {
            return match value {
                Ok(val) => Ok(val),
                Err(e) => Err(crate::error::SyscityError::Validation(format!(
                    "Invalid arguments for tool {}: {}",
                    tool_name, e
                ))),
            };
        }

        Err(crate::error::SyscityError::Validation(format!(
            "Empty arguments for tool {}",
            tool_name
        )))
    }

    /// Execute a function call from an LLM.
    /// Checks both static and dynamic registries.
    /// Enforces the timeout configured in `ToolContext`.
    pub async fn execute_call(
        &self,
        call: &FunctionCall,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let args: Value = self.parse_tool_args(&call.arguments, &call.name)?;
        let tool_name = call.name.clone();
        let timeout = context.timeout();

        // Try static tools first
        if let Some(tool) = self.get(&tool_name) {
            let exec_start = std::time::Instant::now();
            let exec_future = tool.execute(args, context);
            let result: crate::Result<ToolExecutionResult> =
                tokio::time::timeout(timeout, exec_future)
                    .await
                    .map_err(|_| {
                        crate::error::SyscityError::Timeout(format!(
                            "Tool '{}' timed out after {:?} (actual execution: {:?})",
                            tool_name,
                            timeout,
                            exec_start.elapsed()
                        ))
                    })?;
            info!(
                "execute_call: tool={} completed in {:?} (timeout={:?})",
                tool_name,
                exec_start.elapsed(),
                timeout
            );
            return result;
        }

        // Try dynamic tools
        let dynamic_tool = self
            .dynamic_tools
            .read()
            .ok()
            .and_then(|map| map.get(&tool_name).cloned());

        if let Some(tool) = dynamic_tool {
            if !self.is_blocked(&tool_name) && !self.is_degraded(&tool_name) {
                let exec_start = std::time::Instant::now();
                let exec_future = tool.execute(args, context);
                let result: crate::Result<ToolExecutionResult> =
                    tokio::time::timeout(timeout, exec_future)
                        .await
                        .map_err(|_| {
                            crate::error::SyscityError::Timeout(format!(
                                "Tool '{}' timed out after {:?} (actual execution: {:?})",
                                tool_name,
                                timeout,
                                exec_start.elapsed()
                            ))
                        })?;
                info!(
                    "execute_call: tool={} completed in {:?} (timeout={:?})",
                    tool_name,
                    exec_start.elapsed(),
                    timeout
                );
                return result;
            }
        }

        Err(crate::error::SyscityError::Validation(format!(
            "Unknown tool: {}. Available tools: {}",
            tool_name,
            self.list().join(", ")
        )))
    }

    /// Execute a function call from an LLM with streaming output.
    ///
    /// Policy hooks, approval, and before-hooks are run before chunks are
    /// yielded. `on_chunk` is invoked for every [`ToolExecutionChunk`]
    /// produced by the tool. After the stream completes, after-hooks,
    /// content filtering, and audit logging are applied and the final
    /// [`ToolExecutionResult`] is returned.
    ///
    /// This method owns the tool reference internally, so it works for both
    /// static and dynamically-registered tools without lifetime issues.
    pub async fn execute_call_streaming<F, Fut>(
        &self,
        call: &FunctionCall,
        context: &ToolContext,
        mut on_chunk: F,
    ) -> crate::Result<ToolExecutionResult>
    where
        F: FnMut(ToolExecutionChunk) -> Fut + Send,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let args: Value = self.parse_tool_args(&call.arguments, &call.name)?;

        let tool_name = call.name.clone();

        let policy_decision = self.evaluate_policy(&tool_name, &args).await;
        match policy_decision {
            // Allow → proceed to execution below.
            ToolPolicyDecision::Allow => {}
            ToolPolicyDecision::Deny { reason } => {
                return Err(crate::error::SyscityError::Validation(format!(
                    "Tool '{}' denied: {}",
                    tool_name, reason
                )));
            }
            ToolPolicyDecision::NeedsApproval { .. } => {
                // For streaming tools, fall back to buffered execution so the
                // approval flow can suspend and resume in a single future.
                let result = self.execute(&tool_name, args.clone(), context).await;
                return match result {
                    Some(Ok(exec_result)) => {
                        if !exec_result.output.is_empty() {
                            on_chunk(ToolExecutionChunk::Output(exec_result.output.clone())).await;
                        }
                        if let Some(error) = exec_result.error.clone() {
                            on_chunk(ToolExecutionChunk::Error(error)).await;
                        }
                        if let Some(data) = exec_result.data.clone() {
                            on_chunk(ToolExecutionChunk::Data(data)).await;
                        }
                        Ok(exec_result)
                    }
                    Some(Err(e)) => {
                        on_chunk(ToolExecutionChunk::Error(e.to_string())).await;
                        Err(e)
                    }
                    None => Err(crate::error::SyscityError::Validation(format!(
                        "Tool '{}' was found but could not be executed (may have been \
                         deregistered)",
                        tool_name,
                    ))),
                };
            }
        }

        // Run before-hooks.
        self.active_hooks().run_before(&tool_name, &args).await;

        // Look up the tool and consume its stream.
        let collected = if let Some(tool) = self.get(&tool_name) {
            consume_stream(tool.execute_stream(args.clone(), context), &mut on_chunk).await
        } else {
            let dynamic_tool = self
                .dynamic_tools
                .read()
                .ok()
                .and_then(|map| map.get(&tool_name).cloned());
            if let Some(tool) = dynamic_tool {
                if !self.is_blocked(&tool_name) && !self.is_degraded(&tool_name) {
                    consume_stream(tool.execute_stream(args.clone(), context), &mut on_chunk).await
                } else {
                    return Err(crate::error::SyscityError::Validation(format!(
                        "Tool '{}' is blocked or degraded",
                        tool_name
                    )));
                }
            } else {
                return Err(crate::error::SyscityError::Validation(format!(
                    "Unknown tool: {}. Available tools: {}",
                    tool_name,
                    self.list().join(", ")
                )));
            }
        };

        // Apply after-hooks, content filtering, and audit logging.
        match self
            .finalize_stream_result(&tool_name, &args, context, collected)
            .await
        {
            Some(Ok(result)) => Ok(result),
            Some(Err(e)) => Err(e),
            None => Err(crate::error::SyscityError::Validation(format!(
                "Tool '{}' finalization failed",
                tool_name
            ))),
        }
    }

    /// Apply content filtering and audit logging to a collected streaming
    /// result, and run after-hooks.
    ///
    /// This is the streaming equivalent of the post-processing performed by
    /// [`execute`](ToolRegistry::execute) after a buffered call.
    pub async fn finalize_stream_result(
        &self,
        name: &str,
        args: &Value,
        context: &ToolContext,
        collected: ToolExecutionResult,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        self.active_hooks().run_after(name, args, &collected).await;
        self.filter_and_audit(name, context, Some(Ok(collected)))
            .await
    }
}
