//! Structural proposer (§十一 结构化改版提议).
//!
//! An LLM (or a deterministic fallback) produces candidates that rewrite
//! LLM-facing structural text — tool descriptions and the system prompt.
//! Each candidate is judged by the harness gate and **only `Improved`
//! candidates are adopted**; every adoption/rejection lands in
//! `decision_traces` for auditability (§十二 可追溯).
//!
//! Safety is enforced by two fences:
//!
//! - **Search-space fence** — [`fence_path`] rejects candidates whose target
//!   path touches a security-locked field (api key / secret / token /
//!   password / credential). Fenced candidates are never adoptable.
//! - **Harness verdict** — [`StructuralProposer::judge`] only labels a
//!   candidate `Improved` when it is non-empty, actually differs from the
//!   current text, and has evidence (pending badcases referencing the object).
//!
//! Tool descriptions are applied through the shared [`ToolRegistry`] metadata
//! store, so a rewrite reaches every running agent on the next turn without a
//! restart; the system prompt is applied through the same CAS config machinery
//! the scalar optimizer uses.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::error::{Result, SyscityError};
use crate::eval::apply_patch::{apply_optimizer_patch, OptimizerPatch, PatchOutcome};
use crate::eval::comparison::ComparisonVerdict;
use crate::eval::decision_trace::{RecordTraceParams, TraceKind, TraceStatus};
use crate::eval::pending_badcase::PendingStatus;
use crate::gateway::{config_revision, GatewayState};
use crate::providers::{CompletionRequest, Message, Provider};
use crate::tools::ToolRegistry;

/// Which structural object a candidate rewrites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralObjectKind {
    /// An LLM-facing tool description (registry metadata override).
    ToolDescription,
    /// The default agent's system prompt (`default_agent.system_prompt`).
    Prompt,
}

impl StructuralObjectKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ToolDescription => "tool_description",
            Self::Prompt => "prompt",
        }
    }

    /// Parse a snake_case kind string; `None` for unknown kinds (dropped).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tool_description" => Some(Self::ToolDescription),
            "prompt" => Some(Self::Prompt),
            _ => None,
        }
    }
}

/// A structural rewording candidate produced by the proposer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralCandidate {
    pub id: String,
    pub object: StructuralObjectKind,
    /// Target — a tool name for `ToolDescription`, a config path for `Prompt`.
    pub path: String,
    pub current: String,
    pub proposed: String,
    pub reason: String,
    /// Blocked by the search-space fence (security-locked) — never adoptable.
    pub fenced: bool,
    /// Number of pending badcases referencing this object.
    pub evidence: u32,
    /// Harness verdict, filled after [`StructuralProposer::judge`].
    pub verdict: Option<ComparisonVerdict>,
}

/// Result of adopting (or refusing to adopt) a candidate.
#[derive(Debug, Clone, Serialize)]
pub struct AdoptionReport {
    pub candidate_id: String,
    pub adopted: bool,
    pub reason: String,
    pub new_revision: Option<String>,
}

/// §十一 搜索空间圈定 — substrings that mark a path as security-locked.
const SECURITY_MARKERS: &[&str] = &[
    "api_key",
    "apikey",
    "secret",
    "token",
    "password",
    "credential",
    "webhook",
    "ssh_key",
    "oauth",
    "encryption",
    "keyring",
    "jwt",
];

/// Search-space fence: `true` when `path` touches a security-locked field.
/// Fenced candidates are rejected regardless of how good they look.
pub fn fence_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    SECURITY_MARKERS.iter().any(|m| lower.contains(m))
}

/// The structural proposer. Stateless — owns the optional LLM provider and the
/// candidate cap.
pub struct StructuralProposer {
    provider: Option<Arc<dyn Provider + Send + Sync>>,
    /// Hard cap on candidates produced per call (§十一, default 4).
    max_candidates: usize,
}

/// Compact input summary for the LLM, plus per-path badcase reference counts.
struct ProposerContext {
    summary: String,
    references: HashMap<String, u32>,
}

impl StructuralProposer {
    pub fn new(provider: Option<Arc<dyn Provider + Send + Sync>>, max_candidates: usize) -> Self {
        Self {
            provider,
            max_candidates: max_candidates.clamp(1, 8),
        }
    }

    /// Propose structural rewording candidates for the default agent.
    ///
    /// Uses the LLM provider when available and falls back to a deterministic
    /// rule-based generator otherwise. Every candidate is run through the
    /// search-space fence and the harness gate before it is returned.
    pub async fn propose(&self, state: &Arc<GatewayState>) -> Result<Vec<StructuralCandidate>> {
        let ctx = self.build_context(state).await;
        let mut candidates = if let Some(provider) = &self.provider {
            match self.llm_propose(provider, &ctx).await {
                Ok(cands) if !cands.is_empty() => cands,
                Ok(_) => self.rule_based_propose(state, &ctx),
                Err(e) => {
                    debug!("Structural proposer LLM failed ({}); using rule-based fallback", e);
                    self.rule_based_propose(state, &ctx)
                }
            }
        } else {
            self.rule_based_propose(state, &ctx)
        };

        candidates.truncate(self.max_candidates);
        for cand in candidates.iter_mut() {
            cand.fenced = fence_path(&cand.path);
            if !cand.fenced {
                cand.verdict = Some(self.judge(cand));
            }
        }
        Ok(candidates)
    }

    /// Harness gate for a candidate. Only `Improved` candidates are adoptable.
    fn judge(&self, cand: &StructuralCandidate) -> ComparisonVerdict {
        if cand.fenced
            || cand.proposed.trim().is_empty()
            || cand.proposed == cand.current
            || cand.current.trim().is_empty()
        {
            return ComparisonVerdict::NoSignificantChange;
        }
        if cand.evidence > 0 {
            ComparisonVerdict::Improved
        } else {
            ComparisonVerdict::NoSignificantChange
        }
    }

    /// Adopt a candidate, applying it and recording a decision trace.
    ///
    /// Non-`Improved` and fenced candidates are rejected with a trace and no
    /// mutation. Returns a report the caller can surface via `eval.propose`.
    pub async fn adopt(
        &self,
        state: &Arc<GatewayState>,
        cand: &StructuralCandidate,
    ) -> Result<AdoptionReport> {
        if cand.fenced {
            warn!(
                "Structural proposer refuses fenced candidate {} ({}): fence hit on '{}'",
                cand.id,
                cand.object.as_str(),
                cand.path
            );
            self.record_trace(state, TraceKind::OptimizerReject, cand, "fenced")
                .await?;
            return Ok(AdoptionReport {
                candidate_id: cand.id.clone(),
                adopted: false,
                reason: "fenced".to_string(),
                new_revision: None,
            });
        }
        if cand.verdict != Some(ComparisonVerdict::Improved) {
            self.record_trace(state, TraceKind::OptimizerReject, cand, "not_improved")
                .await?;
            return Ok(AdoptionReport {
                candidate_id: cand.id.clone(),
                adopted: false,
                reason: "not_improved".to_string(),
                new_revision: None,
            });
        }

        match cand.object {
            StructuralObjectKind::ToolDescription => {
                // The shared registry propagates the override to running agents
                // on the next turn — no restart or command needed.
                let next_version = state
                    .tools
                    .registry
                    .metadata_for(&cand.path)
                    .map(|m| m.version.saturating_add(1))
                    .unwrap_or(1);
                state.tools.registry.set_metadata(
                    &cand.path,
                    crate::tools::ToolDescriptionMeta::new(next_version, &cand.proposed),
                );
                info!(
                    "Structural proposer adopted tool_description {} (v{})",
                    cand.path, next_version
                );
                self.record_trace(state, TraceKind::OptimizerApply, cand, "improved")
                    .await?;
                Ok(AdoptionReport {
                    candidate_id: cand.id.clone(),
                    adopted: true,
                    reason: "improved".to_string(),
                    new_revision: None,
                })
            }
            StructuralObjectKind::Prompt => {
                let base_revision = {
                    let cfg = state.config.read().await;
                    config_revision(&cfg)
                };
                let patch = OptimizerPatch {
                    path: "default_agent.system_prompt".to_string(),
                    value: json!(cand.proposed),
                };
                match apply_optimizer_patch(state, &patch, &base_revision).await {
                    PatchOutcome::Applied { new_revision } => {
                        info!(
                            "Structural proposer adopted prompt {} -> revision {}",
                            cand.path, new_revision
                        );
                        self.record_trace(state, TraceKind::OptimizerApply, cand, "improved")
                            .await?;
                        Ok(AdoptionReport {
                            candidate_id: cand.id.clone(),
                            adopted: true,
                            reason: "improved".to_string(),
                            new_revision: Some(new_revision),
                        })
                    }
                    PatchOutcome::Conflict { current } => {
                        self.record_trace(
                            state,
                            TraceKind::OptimizerReject,
                            cand,
                            "revision_conflict",
                        )
                        .await?;
                        Ok(AdoptionReport {
                            candidate_id: cand.id.clone(),
                            adopted: false,
                            reason: format!("revision_conflict({current})"),
                            new_revision: None,
                        })
                    }
                    PatchOutcome::UnknownPath => {
                        self.record_trace(state, TraceKind::OptimizerReject, cand, "unknown_path")
                            .await?;
                        Ok(AdoptionReport {
                            candidate_id: cand.id.clone(),
                            adopted: false,
                            reason: "unknown_path".to_string(),
                            new_revision: None,
                        })
                    }
                }
            }
        }
    }

    // ── Candidate generation ────────────────────────────────────────────────

    /// Deterministic fallback: propose a description rewrite for every tool
    /// referenced by pending badcases. No LLM required.
    fn rule_based_propose(
        &self,
        state: &Arc<GatewayState>,
        ctx: &ProposerContext,
    ) -> Vec<StructuralCandidate> {
        let registry = &state.tools.registry;
        let mut out = Vec::new();
        for name in registry.list() {
            let count = ctx.references.get(&name).copied().unwrap_or(0);
            if count == 0 {
                continue;
            }
            let current = Self::tool_description(registry, &name);
            if current.trim().is_empty() {
                continue;
            }
            let proposed = format!(
                "{}. If the request is ambiguous, ask a clarifying question before acting.",
                current
            );
            out.push(StructuralCandidate {
                id: uuid::Uuid::new_v4().to_string(),
                object: StructuralObjectKind::ToolDescription,
                path: name,
                current,
                proposed,
                reason: format!("{count} pending badcase(s) reference this tool"),
                fenced: false,
                evidence: count,
                verdict: None,
            });
        }
        out
    }

    /// LLM generation: ask the provider for up to `max_candidates` rewording
    /// candidates as a JSON array. Any failure degrades to the rule-based
    /// generator.
    async fn llm_propose(
        &self,
        provider: &Arc<dyn Provider + Send + Sync>,
        ctx: &ProposerContext,
    ) -> Result<Vec<StructuralCandidate>> {
        let system = concat!(
            "You are the syscity structural proposer. Given pending badcases and tuning traces, ",
            "propose up to 4 concrete rewrites of LLM-facing text (tool descriptions or the ",
            "system prompt). Respond ONLY with a JSON array of objects shaped as ",
            "{\"object\":\"tool_description\"|\"prompt\",\"path\":\"...\",\"current\":\"...\",",
            "\"proposed\":\"...\",\"reason\":\"...\"}. ",
            "Paths must never contain api_key/secret/token/password/credential."
        );
        let request = CompletionRequest {
            messages: vec![
                Message::system(system.to_string()),
                Message::user(ctx.summary.clone()),
            ],
            model: Some(provider.default_model().to_string()),
            temperature: Some(0.2),
            max_tokens: Some(1024),
            stream: false,
            ..Default::default()
        };
        let response = provider.complete(request).await.map_err(|e| {
            SyscityError::Internal(format!("structural proposer LLM call failed: {e}"))
        })?;
        let items = parse_candidates_json(&response.message.content)?;

        let mut out = Vec::new();
        for item in items {
            let object = match item
                .get("object")
                .and_then(|v| v.as_str())
                .and_then(StructuralObjectKind::parse)
            {
                Some(o) => o,
                None => continue,
            };
            let path = match item.get("path").and_then(|v| v.as_str()) {
                Some(p) if !p.trim().is_empty() => p.trim().to_string(),
                _ => continue,
            };
            let current = item
                .get("current")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let proposed = item
                .get("proposed")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let reason = item
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("proposed by LLM")
                .to_string();
            let evidence = ctx.references.get(&path).copied().unwrap_or(0);
            out.push(StructuralCandidate {
                id: uuid::Uuid::new_v4().to_string(),
                object,
                path,
                current,
                proposed,
                reason,
                fenced: false,
                evidence,
                verdict: None,
            });
        }
        Ok(out)
    }

    /// Current effective description for a tool (metadata override, then
    /// static, then dynamic registration).
    fn tool_description(registry: &ToolRegistry, name: &str) -> String {
        registry.description_for(name).unwrap_or_default()
    }

    /// Gather the input context: pending badcases and recent decision traces,
    /// plus per-path badcase reference counts used by the harness gate.
    async fn build_context(&self, state: &Arc<GatewayState>) -> ProposerContext {
        let mut references = HashMap::new();
        let mut lines = Vec::new();

        if let Some(store) = state.infra.pending_badcase_store.as_ref() {
            if let Ok(badcases) = store.list_pending(PendingStatus::Pending, 200).await {
                for b in badcases.iter().take(100) {
                    let text = format!("{} | {}", b.input, b.response);
                    for name in state.tools.registry.list() {
                        if text.contains(name.as_str()) {
                            *references.entry(name).or_insert(0) += 1;
                        }
                    }
                    for sig in &b.risk_signals {
                        lines.push(format!("badcase[{}] {}", b.id, sig));
                    }
                }
                lines.push(format!("{} pending badcase(s)", badcases.len()));
            }
        }

        if let Some(store) = state.infra.decision_trace_store.as_ref() {
            if let Ok(traces) = store.list(None, 50).await {
                for t in traces.iter().take(20) {
                    lines.push(format!("trace[{}] {}", t.kind.as_str(), t.subject));
                }
            }
        }

        let summary = if lines.is_empty() {
            "no pending badcases or decision traces".to_string()
        } else {
            lines.join("\n")
        };
        ProposerContext { summary, references }
    }

    /// Persist a decision trace for an adoption or rejection.
    async fn record_trace(
        &self,
        state: &Arc<GatewayState>,
        kind: TraceKind,
        cand: &StructuralCandidate,
        outcome: &str,
    ) -> Result<()> {
        let Some(store) = state.infra.decision_trace_store.as_ref() else {
            return Ok(()); // Trace store is optional; adoption itself already happened.
        };
        let payload = json!({
            "object": cand.object.as_str(),
            "path": cand.path,
            "from": cand.current,
            "to": cand.proposed,
            "reason": cand.reason,
        });
        let evidence = json!({
            "outcome": outcome,
            "verdict": cand.verdict.as_ref().map(|v| format!("{:?}", v)),
            "evidence_count": cand.evidence,
        });
        store
            .record(&RecordTraceParams {
                kind,
                subject: format!("struct:{}:{}", cand.object.as_str(), cand.path),
                payload,
                evidence,
                status: if kind == TraceKind::OptimizerApply {
                    TraceStatus::Applied
                } else {
                    TraceStatus::Rejected
                },
            })
            .await
            .map_err(|e| SyscityError::Storage {
                context: "failed to record structural proposer decision trace".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }
}

/// Recover a JSON array of candidates from an LLM response, tolerating code
/// fences and surrounding prose (first `[` .. last `]`).
fn parse_candidates_json(raw: &str) -> Result<Vec<Value>> {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<Vec<Value>>(trimmed) {
        return Ok(v);
    }
    let stripped = strip_code_fences(trimmed);
    if let Ok(v) = serde_json::from_str::<Vec<Value>>(stripped) {
        return Ok(v);
    }
    let Some(start) = stripped.find('[') else {
        return Err(SyscityError::Internal(
            "structural proposer: no JSON array found in LLM response".to_string(),
        ));
    };
    let Some(end) = stripped.rfind(']') else {
        return Err(SyscityError::Internal(
            "structural proposer: no JSON array found in LLM response".to_string(),
        ));
    };
    if end <= start {
        return Err(SyscityError::Internal(
            "structural proposer: no JSON array found in LLM response".to_string(),
        ));
    }
    serde_json::from_str::<Vec<Value>>(&stripped[start..=end]).map_err(|e| {
        SyscityError::Internal(format!("structural proposer: bad candidate JSON: {e}"))
    })
}

fn strip_code_fences(s: &str) -> &str {
    let s = s.trim();
    let start = s.find('`').map(|i| i + 1).unwrap_or(0);
    let end = s.rfind('`').unwrap_or(s.len());
    if start < end {
        &s[start..end]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::{make_test_state, make_test_state_with_store};
    use crate::gateway::GatewayConfig;
    use crate::tools::{Tool, ToolContext, ToolExecutionResult};

    // Minimal stub tool named "file_write" — the rule-based proposer only
    // reads `name()`/`description()`, so the body can be trivial.
    struct StubFileWriteTool;

    #[async_trait::async_trait]
    impl Tool for StubFileWriteTool {
        fn name(&self) -> &str {
            "file_write"
        }
        fn description(&self) -> &str {
            "write a file to disk"
        }
        fn parameters_schema(&self) -> Value {
            json!({"type": "object", "properties": {"path": {"type": "string"}}})
        }
        async fn execute(
            &self,
            _args: Value,
            _context: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            Ok(ToolExecutionResult::success("ok"))
        }
    }

    #[test]
    fn fence_blocks_security_paths() {
        assert!(fence_path("provider.api_key"));
        assert!(fence_path("webhook_secret"));
        assert!(fence_path("default_agent.credential"));
        assert!(fence_path("ssh_key_path"));
        assert!(!fence_path("file_write"));
        assert!(!fence_path("default_agent.system_prompt"));
    }

    #[test]
    fn object_kind_roundtrips() {
        assert_eq!(
            StructuralObjectKind::parse("tool_description"),
            Some(StructuralObjectKind::ToolDescription)
        );
        assert_eq!(StructuralObjectKind::parse("prompt"), Some(StructuralObjectKind::Prompt));
        assert_eq!(StructuralObjectKind::parse("sop"), None);
        assert_eq!(StructuralObjectKind::ToolDescription.as_str(), "tool_description");
    }

    #[test]
    fn judge_requires_evidence_and_change() {
        let proposer = StructuralProposer::new(None, 4);
        let base = StructuralCandidate {
            id: "c1".into(),
            object: StructuralObjectKind::ToolDescription,
            path: "file_write".into(),
            current: "write a file".into(),
            proposed: "write a file safely".into(),
            reason: "evidence".into(),
            fenced: false,
            evidence: 0,
            verdict: None,
        };
        // No evidence → not improved.
        assert_eq!(proposer.judge(&base), ComparisonVerdict::NoSignificantChange);

        let with_evidence = StructuralCandidate { evidence: 2, ..base.clone() };
        assert_eq!(proposer.judge(&with_evidence), ComparisonVerdict::Improved);

        // No actual change → not improved.
        let unchanged = StructuralCandidate {
            proposed: "write a file".into(),
            evidence: 3,
            ..base.clone()
        };
        assert_eq!(proposer.judge(&unchanged), ComparisonVerdict::NoSignificantChange);

        // Fenced → never improved.
        let fenced = StructuralCandidate {
            fenced: true,
            evidence: 3,
            ..base
        };
        assert_eq!(proposer.judge(&fenced), ComparisonVerdict::NoSignificantChange);
    }

    #[tokio::test]
    async fn no_store_no_candidates_is_empty() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let proposer = StructuralProposer::new(None, 4);
        let cands = proposer.propose(&state).await.unwrap();
        assert!(cands.is_empty(), "no badcases and no tools -> no candidates");
    }

    #[tokio::test]
    async fn rule_based_proposes_and_adopts_tool_description() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        // Register a tool the badcase will reference.
        state
            .tools
            .registry
            .register_dynamic(std::sync::Arc::new(StubFileWriteTool));

        // Seed a pending badcase referencing the tool.
        let store = state.infra.pending_badcase_store.as_ref().unwrap();
        let params = crate::eval::InsertPendingParams {
            source: crate::eval::PendingSource::OnlineRisk,
            turn_id: None,
            session_id: None,
            agent_id: None,
            input: "use the file_write tool to save the notes".to_string(),
            response: "ok".to_string(),
            risk_signals: vec!["unhelpful_tool_usage".to_string()],
        };
        store.insert_pending(&params).await.unwrap();

        let proposer = StructuralProposer::new(None, 4);
        let cands = proposer.propose(&state).await.unwrap();
        assert!(
            cands.iter().any(|c| c.path == "file_write"),
            "expected a file_write candidate, got {:?}",
            cands.iter().map(|c| c.path.clone()).collect::<Vec<_>>()
        );
        let cand = cands.iter().find(|c| c.path == "file_write").unwrap();
        assert_eq!(cand.verdict, Some(ComparisonVerdict::Improved));
        assert!(!cand.fenced);

        let report = proposer.adopt(&state, cand).await.unwrap();
        assert!(report.adopted, "Improved candidate must be adopted");
        assert!(state.tools.registry.metadata_for("file_write").is_some());

        // Decision trace recorded.
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].kind, TraceKind::OptimizerApply);
        assert!(traces[0]
            .subject
            .starts_with("struct:tool_description:file_write"));
    }

    #[tokio::test]
    async fn fenced_candidate_is_refused_with_trace() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let proposer = StructuralProposer::new(None, 4);
        let cand = StructuralCandidate {
            id: "evil".into(),
            object: StructuralObjectKind::Prompt,
            path: "default_agent.credential".into(),
            current: "x".into(),
            proposed: "y".into(),
            reason: "test".into(),
            fenced: true,
            evidence: 5,
            verdict: Some(ComparisonVerdict::Improved), // even a "good" fenced candidate is refused
        };
        let report = proposer.adopt(&state, &cand).await.unwrap();
        assert!(!report.adopted, "fence must block adoption");
        assert_eq!(report.reason, "fenced");
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].kind, TraceKind::OptimizerReject);
        assert_eq!(traces[0].evidence["outcome"], "fenced");
    }

    #[tokio::test]
    async fn not_improved_candidate_is_rejected() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        // Pin the system prompt so the no-change candidate is well-defined.
        {
            let mut guard = state.config.write().await;
            Arc::make_mut(&mut guard).default_agent.system_prompt = "be helpful".to_string();
        }
        let proposer = StructuralProposer::new(None, 4);
        let cand = StructuralCandidate {
            id: "weak".into(),
            object: StructuralObjectKind::Prompt,
            path: "default_agent.system_prompt".into(),
            current: "be helpful".into(),
            proposed: "be helpful".into(), // no change
            reason: "test".into(),
            fenced: false,
            evidence: 3,
            verdict: Some(ComparisonVerdict::NoSignificantChange),
        };
        let report = proposer.adopt(&state, &cand).await.unwrap();
        assert!(!report.adopted);
        assert_eq!(report.reason, "not_improved");
        // Nothing mutated.
        assert_eq!(state.config.read().await.default_agent.system_prompt, "be helpful");
        let traces = state
            .infra
            .decision_trace_store
            .as_ref()
            .unwrap()
            .list(None, 10)
            .await
            .unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].kind, TraceKind::OptimizerReject);
    }

    #[tokio::test]
    async fn prompt_candidate_applies_system_prompt_via_cas() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let proposer = StructuralProposer::new(None, 4);
        let cand = StructuralCandidate {
            id: "p1".into(),
            object: StructuralObjectKind::Prompt,
            path: "default_agent.system_prompt".into(),
            current: "be helpful".into(),
            proposed: "be helpful and concise".into(),
            reason: "clarity".into(),
            fenced: false,
            evidence: 1,
            verdict: Some(ComparisonVerdict::Improved),
        };
        let report = proposer.adopt(&state, &cand).await.unwrap();
        assert!(report.adopted, "Improved prompt must be adopted");
        assert!(report.new_revision.is_some());
        assert_eq!(state.config.read().await.default_agent.system_prompt, "be helpful and concise");
    }
}
