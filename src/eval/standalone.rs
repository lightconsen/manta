//! Standalone eval mode — creates Agent + Critic inline (no daemon needed).
//!
//! Entry point: [`run_standalone_suite`].
//!
//! ```bash
//! syscity eval run ci_smoke --full --trials 3
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use rand::seq::SliceRandom;
use tracing::{info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::reflection::critic::Critic;
use crate::agent::Agent;
use crate::eval::harness::{EarlyStopConfig, EvalHarness};
use crate::eval::loader::{default_evals_dir, load_suite};
use crate::eval::rca::RcaPipeline;
use crate::eval::recycle::{extract_rca_results_from_badcases, BadcaseCollector};
use crate::eval::EvalTask;
use crate::eval::{
    generate_action_items, write_action_items, HumanReviewStore, LayeredScorer, ScorerConfig,
};
use crate::gateway::config::GatewayConfig;
use crate::providers::resolver::resolve_provider;
use crate::tools::file::{FileEditTool, FileReadTool, FileWriteTool, GlobTool};
use crate::tools::grep::GrepTool;
use crate::tools::shell::ShellTool;
use crate::tools::time::TimeTool;
use crate::tools::todo_tool::TodoTool;
use crate::tools::web::{WebFetchTool, WebSearchTool};
use crate::tools::{AcpSessionTool, AcpSpawnTool, SessionsSendTool, ToolRegistry};
use crate::Result;

/// Which provider a CLI-driven eval run should use; every field left as
/// `None` falls back to the daemon's configured provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderSelection {
    /// Provider preset/type override.
    pub provider: Option<String>,
    /// Model override.
    pub model: Option<String>,
    /// API key override.
    pub api_key: Option<String>,
    /// API base URL override.
    pub base_url: Option<String>,
}

/// Tunables for a standalone suite run beyond what the suite manifest says.
#[derive(Debug, Clone, Default)]
pub struct SuiteRunOptions {
    /// Eval directory override (defaults to the standard evals dir).
    pub evals_dir: Option<PathBuf>,
    /// Trial-count override.
    pub trials_override: Option<usize>,
    /// Sampling-rate override.
    pub sampling_rate_override: Option<f64>,
    /// Print per-skill score breakdowns.
    pub skill_breakdown: bool,
    /// Collect bad cases into the badcase store.
    pub collect_badcases: bool,
    /// Judge (Critic) provider overrides. All-`None` judges with the agent's
    /// own provider (self-evaluation); setting any field separates the judge.
    pub judge: ProviderSelection,
}

/// Run a full eval suite standalone (no daemon needed).
///
/// 1. Loads GatewayConfig from the config file (or env var fallback).
/// 2. Creates a provider, tool registry, Agent, and optional Critic.
/// 3. Runs `EvalHarness` for each task in the suite.
/// 4. Prints per-task and summary results.
pub async fn run_standalone_suite(
    _config: &crate::config::Config,
    suite_name: &str,
    opts: SuiteRunOptions,
    selection: ProviderSelection,
) -> Result<()> {
    let SuiteRunOptions {
        evals_dir: evals_dir_override,
        trials_override,
        sampling_rate_override,
        skill_breakdown,
        collect_badcases,
        judge,
    } = opts;
    let ProviderSelection {
        provider: provider_override,
        model: model_override,
        api_key: api_key_override,
        base_url: base_url_override,
    } = selection;
    let evals_dir = evals_dir_override.unwrap_or_else(default_evals_dir);
    let manifest_path = evals_dir
        .join("suites")
        .join(format!("{}.yaml", suite_name));

    if !manifest_path.exists() {
        return Err(crate::error::SyscityError::Validation(format!(
            "Suite '{}' not found at {:?}",
            suite_name, manifest_path
        )));
    }

    let suite = load_suite(&manifest_path, suite_name)?;

    println!("\n═══ Eval Suite: {} ═══", suite.name);
    println!("  Tasks:    {}", suite.tasks.len());
    println!("  Min pass: {:.0}%", suite.min_pass_rate * 100.0);
    println!("  Trials:   {}\n", trials_override.unwrap_or(suite.trials));

    // ── Step 1: Try to load GatewayConfig from the config file ──────────────
    let gateway_cfg = try_load_gateway_config().await;
    // Human-review sampling rate (§三): when configured, ordinary (non
    // low-confidence/conflict) cases are additionally routed to review.
    let human_review_rate = gateway_cfg
        .as_ref()
        .and_then(|c| c.eval.human_review.sampling_rate);

    // ── Step 2: Resolve provider config ─────────────────────────────────────
    let provider_type = provider_override.clone().unwrap_or_else(|| {
        gateway_cfg
            .as_ref()
            .map(|g| g.model_provider.clone())
            .unwrap_or_else(|| "anthropic".to_string())
    });

    let model = model_override
        .clone()
        .or_else(|| gateway_cfg.as_ref().map(|g| g.model.clone()));

    let api_key = match api_key_override {
        Some(k) => Some(k),
        None => {
            // Try the configured provider's api_key from GatewayConfig
            match gateway_cfg
                .as_ref()
                .and_then(|g| g.providers.get(&provider_type))
            {
                Some(p) => {
                    let key = p.effective_key().await;
                    if key.is_empty() {
                        None
                    } else {
                        Some(key)
                    }
                }
                None => None,
            }
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        }
    };

    let base_url = base_url_override.or_else(|| {
        gateway_cfg
            .as_ref()
            .and_then(|g| g.providers.get(&provider_type))
            .and_then(|p| p.base_url.clone())
            .or_else(|| std::env::var("SYSCITY_BASE_URL").ok())
    });

    // ── Step 3: Create provider ─────────────────────────────────────────────
    // (keys/URL are cloned so the judge fallback below can reuse them)
    let provider = match resolve_provider(
        &provider_type,
        api_key.clone(),
        base_url.clone(),
        model.clone(),
        None,
    ) {
        Ok(p) => p,
        Err(e) => {
            return Err(crate::error::SyscityError::Validation(format!(
                "Failed to create provider '{}': {}\n\
                 Set ANTHROPIC_API_KEY or OPENAI_API_KEY env var, or provide --api-key",
                provider_type, e
            )));
        }
    };

    info!(
        "Eval provider: {} (model: {})",
        provider_type,
        model.as_deref().unwrap_or("default")
    );

    // ── Step 4: Create ACP control plane (for subagent spawning) ────────────
    let acp = Arc::new(AcpControlPlane::new(50));

    // ── Step 5: Create tool registry ────────────────────────────────────────
    let tool_registry =
        create_eval_tool_registry(Some(acp.clone()), gateway_cfg.as_ref().map(|g| &g.search));
    let tool_registry = Arc::new(tool_registry);

    // ── Step 6: Set up agent builder on ACP (needed for acp_spawn) ──────────
    {
        let tools_for_builder = tool_registry.clone();
        let provider_for_builder = provider.clone();
        acp.set_agent_builder(move |subagent_id| {
            let cfg = crate::agent::AgentConfig {
                agent_id: Some(subagent_id.to_string()),
                ..crate::agent::AgentConfig::default()
            };
            Ok(Agent::new(cfg, provider_for_builder.clone(), tools_for_builder.clone()))
        })
        .await;
    }

    // ── Step 7: Create Agent ────────────────────────────────────────────────
    let mut agent_config = gateway_cfg
        .as_ref()
        .map(|g| g.default_agent.clone())
        .unwrap_or_default();
    agent_config.system_prompt.push_str(
        "\n\n## Tool Usage Guidelines\n\n\
         Always prefer using dedicated tools over shell commands. \
         Each tool is purpose-built for its specific task. When a dedicated \
         tool exists for what the user is asking, use it instead of the shell. \
         For example: use 'pdf' for PDF generation, 'image' for image viewing, \
         'image_generate' for image creation, 'tts' for text-to-speech, \
         'stt' for transcription, and 'browser' for web automation.",
    );

    let agent = Arc::new(Agent::new(agent_config, provider.clone(), tool_registry.clone()));

    // ── Step 6: Create Critic (only if at least one task has criteria) ──────
    let has_criteria = suite.tasks.iter().any(|t| t.criteria.is_some());
    let critic = if has_criteria {
        // Judge separation: a different provider needs its own resolved
        // provider; model-only overrides reuse the agent's provider with
        // `with_model`. Key/URL fall back to the agent provider's when the
        // judge uses the same preset.
        let judge_needs_own_provider =
            judge.provider.is_some() || judge.api_key.is_some() || judge.base_url.is_some();
        let (judge_provider, judge_model) = if judge_needs_own_provider {
            let jt = judge
                .provider
                .clone()
                .unwrap_or_else(|| provider_type.clone());
            let same = jt == provider_type;
            let jkey = judge.api_key.clone().or_else(|| {
                if same {
                    api_key.clone()
                } else {
                    std::env::var("ANTHROPIC_API_KEY")
                        .ok()
                        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                }
            });
            let jbase = judge
                .base_url
                .clone()
                .or_else(|| if same { base_url.clone() } else { None });
            let jm = judge.model.clone();
            let jp = resolve_provider(&jt, jkey, jbase, jm.clone(), None).map_err(|e| {
                crate::error::SyscityError::Validation(format!(
                    "Failed to create judge provider '{}': {}",
                    jt, e
                ))
            })?;
            info!("Eval judge provider: {} (model: {})", jt, jm.as_deref().unwrap_or("default"));
            (jp, jm)
        } else {
            (provider.clone(), judge.model.clone().or_else(|| model.clone()))
        };
        let mut c = Critic::new(judge_provider);
        if let Some(ref m) = judge_model {
            c = c.with_model(m.clone());
        }
        Some(c)
    } else {
        None
    };

    // ── Step 6b: Create optional RcaPipeline for badcase collection ─────────
    let rca_pipeline = if collect_badcases {
        critic
            .clone()
            .map(|c| Arc::new(RcaPipeline::new(agent.clone(), Some(c))))
    } else {
        None
    };

    // ── Step 7: Build harness ───────────────────────────────────────────────
    let effective_trials = trials_override.unwrap_or(suite.trials);
    let early_stop = EarlyStopConfig {
        min_trials: 3,
        consecutive_passes: 0,
        consecutive_failures: 0,
        continuous_success_required: suite.continuous_success_required,
    };
    let mut harness = EvalHarness::new(agent.clone(), critic)
        .with_default_trials(effective_trials)
        .with_skill_designs(suite.skill_designs.clone())
        .with_rca_pipeline(rca_pipeline.clone())
        .with_early_stop(early_stop);

    // Attach the layered scorer's routing hook only when a fixed sampling rate
    // is configured. Clean passes / deterministic all-fails route with the
    // configured probability; mixed conditions or judge-flagged trials route
    // unconditionally (base_reason=true) via `route_scored` in the harness.
    if let Some(rate) = human_review_rate {
        if rate > 0.0 {
            let scorer = LayeredScorer::new(None, ScorerConfig::default())
                .with_review_store(HumanReviewStore::new(&evals_dir))
                .with_sampling_rate(Some(rate));
            harness = harness.with_scorer(scorer);
            println!(
                "  Human review: sampling {:.0}% -> {}",
                rate * 100.0,
                evals_dir.join("review").display()
            );
        }
    }

    // ── Step 8: Apply sampling rate (§10) ───────────────────────────────────
    let effective_sampling_rate = sampling_rate_override.unwrap_or(suite.sampling_rate);
    let tasks_to_run: Vec<EvalTask> = if effective_sampling_rate < 1.0 {
        let n = (suite.tasks.len() as f64 * effective_sampling_rate).ceil() as usize;
        let n = n.max(1).min(suite.tasks.len());
        let mut rng = rand::thread_rng();
        let mut indices: Vec<usize> = (0..suite.tasks.len()).collect();
        indices.shuffle(&mut rng);
        indices.truncate(n);
        let mut selected: Vec<EvalTask> = indices.iter().map(|i| suite.tasks[*i].clone()).collect();
        selected.sort_by_key(|t| t.id.clone());
        selected
    } else {
        suite.tasks.clone()
    };

    if tasks_to_run.len() < suite.tasks.len() {
        println!(
            "  Sampling: {}/{} tasks (rate={:.0}%)",
            tasks_to_run.len(),
            suite.tasks.len(),
            effective_sampling_rate * 100.0,
        );
    }

    // ── Step 9: Run each task ───────────────────────────────────────────────
    let mut all_passed = true;
    let mut total_trials = 0usize;
    let mut total_passed = 0usize;

    for task in &tasks_to_run {
        println!("── Task: {} — {} ──", task.id, task.description);
        // A task-level trial override (e.g. difficulty-weighted trials from
        // `load_governed_badcase_suite`) wins over the suite default.
        let task_trials = task.trials.unwrap_or(effective_trials);
        print!("  Running {} trials...", task_trials);
        // Flush stdout so the message appears before potentially long execution
        use std::io::Write;
        let _ = std::io::stdout().flush();

        match harness.run(task.clone(), task_trials).await {
            Ok(summary) => {
                println!(" done\n");
                println!("{}", summary);

                // ── Skill breakdown (verbose) ──
                if skill_breakdown {
                    for trial in &summary.per_trial {
                        if let Some(sr) = &trial.skill_results {
                            if !sr.trigger_results.is_empty()
                                || !sr.execution_results.is_empty()
                                || !sr.quality_results.is_empty()
                                || !sr.resilience_results.is_empty()
                            {
                                println!("  Trial #{} skill details:", trial.trial_index);
                                for r in &sr.trigger_results {
                                    let icon = if r.passed { "✓" } else { "✗" };
                                    println!(
                                        "    {} Trigger [{}]: {}",
                                        icon, r.case_label, r.detail
                                    );
                                }
                                for r in &sr.execution_results {
                                    let icon = if r.passed { "✓" } else { "✗" };
                                    println!(
                                        "    {} Execution [{}]: {}",
                                        icon, r.scenario, r.detail
                                    );
                                }
                                for r in &sr.quality_results {
                                    let icon = if r.passed { "✓" } else { "✗" };
                                    println!("    {} Quality [{}]: {}", icon, r.name, r.detail);
                                }
                                for r in &sr.resilience_results {
                                    let icon = if r.passed { "✓" } else { "✗" };
                                    println!("    {} Resilience: {}", icon, r.detail);
                                }
                            }
                        }
                    }
                }

                // ── Badcase collection ──
                if collect_badcases {
                    let collector = BadcaseCollector::new(
                        rca_pipeline.clone(),
                        Some(evals_dir.join("badcases")),
                    );
                    match collector.collect(&summary, task).await {
                        Ok(n) => {
                            if n > 0 {
                                info!("Collected {} badcases for task '{}'", n, task.id);
                            }
                        }
                        Err(e) => warn!("Badcase collection failed: {}", e),
                    }

                    // ── Action items from RCA results ──
                    match extract_rca_results_from_badcases(&evals_dir) {
                        Ok(results) => {
                            if !results.is_empty() {
                                let items = generate_action_items(&results);
                                if let Err(e) =
                                    write_action_items(&items, &evals_dir.join("actions"))
                                {
                                    warn!("Failed to write action items: {}", e);
                                } else {
                                    info!("Generated {} action items", items.len());
                                }
                            }
                        }
                        Err(e) => warn!("Failed to extract RCA results for action items: {}", e),
                    }
                }

                total_trials += summary.total_trials;
                total_passed += (summary.pass_rate * summary.total_trials as f64).round() as usize;
                if summary.pass_rate < suite.min_pass_rate {
                    all_passed = false;
                }
            }
            Err(e) => {
                println!(" failed\n");
                warn!("Task '{}' failed: {}", task.id, e);
                all_passed = false;
            }
        }
        println!();
    }

    // ── Step 9: Summary ────────────────────────────────────────────────────
    let overall_pass_rate = if total_trials > 0 {
        total_passed as f64 / total_trials as f64
    } else {
        0.0
    };

    println!("═══ Suite Summary: {} ═══", suite.name);
    println!("  Overall pass rate: {:.1}%", overall_pass_rate * 100.0);
    println!("  Total trials:      {}", total_trials);
    println!("  Total passed:      {}", total_passed);
    println!("  Min required:      {:.0}%", suite.min_pass_rate * 100.0);
    println!("  Result:            {}", if all_passed { "PASS" } else { "FAIL" });

    // ── Step 10: Shutdown ──────────────────────────────────────────────────
    agent.shutdown().await?;

    if !all_passed {
        return Err(crate::error::SyscityError::Validation(
            "Suite did not meet minimum pass rate".into(),
        ));
    }

    Ok(())
}

/// Find and parse the config file as `GatewayConfig`.
///
/// Searches the same locations as the daemon:
/// 1. `./config.toml` (CWD)
/// 2. `.config/config.toml` (CWD)
/// 3. `~/.syscity/config.toml`
///
/// Returns `None` if no config file is found or parsing fails.
async fn try_load_gateway_config() -> Option<GatewayConfig> {
    let candidates = [
        PathBuf::from("config.toml"),
        PathBuf::from(".config/config.toml"),
        crate::dirs::default_config_file(),
    ];

    for path in &candidates {
        if path.exists() {
            match tokio::fs::read_to_string(path).await {
                Ok(content) => match toml::from_str::<GatewayConfig>(&content) {
                    Ok(cfg) => {
                        info!("Loaded GatewayConfig from {:?}", path);
                        return Some(cfg);
                    }
                    Err(e) => {
                        warn!("Failed to parse {:?} as GatewayConfig: {}", path, e);
                        // Continue to next candidate
                    }
                },
                Err(e) => {
                    warn!("Failed to read {:?}: {}", path, e);
                }
            }
        }
    }

    info!("No GatewayConfig found — using env var defaults");
    None
}

/// Create a tool registry for eval tasks.
///
/// Registers shell, file operations, search, web, todo, time tools, and
/// optionally ACP tools (acp_spawn, acp_session, sessions_send) when an
/// `AcpControlPlane` is provided.
fn create_eval_tool_registry(
    acp: Option<Arc<AcpControlPlane>>,
    search_cfg: Option<&crate::gateway::config::SearchConfig>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ShellTool::new()));
    registry.register(Box::new(FileReadTool::new()));
    registry.register(Box::new(FileWriteTool::new()));
    registry.register(Box::new(FileEditTool::new()));
    registry.register(Box::new(GrepTool::new()));
    registry.register(Box::new(GlobTool::new()));
    let search_providers = eval_search_providers(search_cfg);
    let shared = std::sync::Arc::new(tokio::sync::RwLock::new(search_providers));
    registry.register(Box::new(WebSearchTool::new().with_providers_arc(shared)));
    registry.register(Box::new(WebFetchTool::new()));
    registry.register(Box::new(TodoTool::new()));
    registry.register(Box::new(TimeTool::new()));

    if let Some(acp) = acp {
        registry.register(Box::new(AcpSpawnTool::new(acp.clone(), None)));
        registry.register(Box::new(AcpSessionTool::new(acp.clone())));
        registry.register(Box::new(SessionsSendTool::new(acp)));
    }

    registry
}

/// Pick search providers for the eval agent: an explicit `[web_search]`
/// config wins; otherwise any `*_API_KEY` env var for a known provider
/// enables it (CI path — DuckDuckGo HTML scraping is blocked on datacenter
/// IPs); DuckDuckGo remains the last-resort fallback.
fn eval_search_providers(
    search_cfg: Option<&crate::gateway::config::SearchConfig>,
) -> Vec<crate::tools::web::SearchProvider> {
    use crate::tools::web::SearchProvider;

    if let Some(cfg) = search_cfg {
        return cfg.to_providers();
    }

    const ENV_KEYS: &[(&str, &str)] = &[
        ("tavily", "TAVILY_API_KEY"),
        ("brave", "BRAVE_API_KEY"),
        ("serper", "SERPER_API_KEY"),
        ("exa", "EXA_API_KEY"),
        ("firecrawl", "FIRECRAWL_API_KEY"),
        ("bocha", "BOCHA_API_KEY"),
        ("serpapi", "SERPAPI_API_KEY"),
    ];
    let mut providers: Vec<SearchProvider> = ENV_KEYS
        .iter()
        .filter_map(|(name, var)| {
            std::env::var(var)
                .ok()
                .filter(|k| !k.trim().is_empty())
                .and_then(|k| SearchProvider::from_config_name(name, Some(k)))
        })
        .collect();
    if !providers.is_empty() {
        providers.push(SearchProvider::DuckDuckGo);
        return providers;
    }

    vec![SearchProvider::DuckDuckGo]
}
