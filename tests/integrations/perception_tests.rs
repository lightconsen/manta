//! Integration tests for the perception fusion layer.
//!
//! These tests validate the full flow: register mock perception sources,
//! poll observations, query entities by modality/source, and exercise the
//! PerceptionQueryTool.

use async_trait::async_trait;
use syscity::computer::{
    ActionResult, ComputerAdapter, DesktopAction, HeadlessComputerAdapter, Rect, Screenshot,
    UiElement, WaitCondition,
};
use syscity::computer::system::SystemMonitor;
use syscity::perception::mock::MockPerceptionSource;
use syscity::perception::{
    AdapterConfig, AdapterError, AggregationStrategy, AgentPerceptionAdapter, Event, Focus,
    Modality, Observation, ObservationId, PerceptionContext, PerceptionContextConfig,
    PerceptionQuery, PerceptionQueryTool, PerceptionRegistry, PerceptionSource,
    PerceptionSummarizer, ScreenshotAdapter, SystemMonitorAdapter,
};
use syscity::tools::{Tool, ToolContext, ToolRegistry};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::Mutex;

/// Create a registry with mock sources.
async fn setup_registry() -> Arc<PerceptionRegistry> {
    let reg = Arc::new(PerceptionRegistry::new(AggregationStrategy::Latest, 10));

    reg.register_source(Arc::new(
        MockPerceptionSource::new("camera")
            .with_modality(Modality::Rgb)
            .with_data(serde_json::json!({"width": 1920, "height": 1080})),
    ))
    .await;

    reg.register_source(Arc::new(
        MockPerceptionSource::new("system_monitor")
            .with_modality(Modality::System)
            .with_data(serde_json::json!({"cpu": 45.0, "memory": 60.0})),
    ))
    .await;

    reg.register_source(Arc::new(
        MockPerceptionSource::new("device:sensor-01:temperature")
            .with_modality(Modality::Device)
            .with_data(serde_json::json!({"celsius": 23.5})),
    ))
    .await;

    reg
}

#[tokio::test]
async fn perception_multiple_sources() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let q = PerceptionQuery::default();
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 3, "expected 3 entities from 3 sources");

    let sources: Vec<String> = result.entities.iter().map(|e| e.id.to_string()).collect();
    assert!(sources.contains(&"camera".to_string()));
    assert!(sources.contains(&"system_monitor".to_string()));
    assert!(sources.contains(&"device:sensor-01:temperature".to_string()));
}

#[tokio::test]
async fn perception_query_by_modality() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let q = PerceptionQuery { modalities: Some(vec![Modality::System]), ..Default::default() };
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 1, "expected 1 system entity");
    assert_eq!(result.entities[0].modality, Modality::System);
}

#[tokio::test]
async fn perception_query_by_source() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let mut q = PerceptionQuery { sources: Some(vec!["camera".to_string()]), ..Default::default() };
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 1, "expected 1 camera entity");
    assert_eq!(result.entities[0].id.to_string(), "camera");

    q.sources = Some(vec!["nonexistent".to_string()]);
    let result = reg.query(&q).await;
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn perception_query_by_label() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let q = PerceptionQuery { label_contains: Some("Rgb".to_string()), ..Default::default() };
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 1, "expected 1 entity with label containing 'Rgb'");
}

#[tokio::test]
async fn perception_query_with_limit() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let q = PerceptionQuery { limit: Some(2), ..Default::default() };
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 2, "expected 2 entities (limited)");
}

#[tokio::test]
async fn perception_query_no_matches() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let q = PerceptionQuery { modalities: Some(vec![Modality::Audio]), ..Default::default() };
    let result = reg.query(&q).await;
    assert!(result.entities.is_empty(), "expected no entities for Audio modality");
}

#[tokio::test]
async fn perception_query_tool_execution() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let tool = PerceptionQueryTool::new(reg.clone());
    let ctx = ToolContext::new("test_user", "test-perception");

    let args = serde_json::json!({"modalities": ["system"]});
    let result = tool.execute(args, &ctx).await.unwrap();
    assert!(result.success, "perception_query tool should succeed");
    assert!(result.output.contains("System"), "output should contain System entity");

    let args = serde_json::json!({});
    let result = tool.execute(args, &ctx).await.unwrap();
    assert!(result.success);
    assert!(result.output.contains("camera"), "output should contain camera");
    assert!(result.output.contains("system_monitor"), "output should contain system_monitor");
}

// ---------------------------------------------------------------------------
// Pipeline E2E — exercises the post-pipeline path:
//   raw_hub → TemporalProcessor / FusionEngine → DerivedStreamHub
//          → AttentionGate → SalienceFilter → MinimalAdapter
// against a "mock agent" that consumes Snapshot + next_event() + summarize().
// ---------------------------------------------------------------------------

/// Stub summariser that confirms `summarize()` reaches an LLM-shaped sink
/// without depending on a real model router.
struct EchoSummarizer;

#[async_trait]
impl PerceptionSummarizer for EchoSummarizer {
    async fn summarize(&self, _system: &str, user: &str) -> Result<String, AdapterError> {
        // Return a fingerprint that proves the user prompt got through.
        let len = user.len();
        Ok(format!("echo:{len}-bytes"))
    }
}

fn raw_obs(source: &str, modality: Modality, data: serde_json::Value) -> Observation {
    Observation {
        id: ObservationId::new(),
        source: source.to_string(),
        modality,
        timestamp: Instant::now(),
        created_at: SystemTime::now(),
        confidence: 1.0,
        data,
    }
}

#[tokio::test]
async fn pipeline_e2e_change_event_reaches_mock_agent() {
    // Mock agent: only cares about System modality, with a 5% delta threshold.
    let focus = Focus::default()
        .with_modalities([Modality::System])
        .with_delta_threshold(Modality::System, 5.0);

    let ctx = PerceptionContext::start(PerceptionContextConfig::default());
    let adapter = ctx.new_adapter(focus, None, AdapterConfig::default());

    // Wire a streaming source.
    let (mock, tx) = MockPerceptionSource::new("cpu").with_streaming(64);
    ctx.raw_hub()
        .attach_source("cpu", Arc::new(mock) as Arc<dyn PerceptionSource>)
        .await;

    // Baseline: 10%.
    tx.send(raw_obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 10.0})))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Salient jump: 50% — should trigger Event::Change.
    tx.send(raw_obs("cpu", Modality::System, serde_json::json!({"cpu_pct": 50.0})))
        .unwrap();

    // The fusion engine may emit Entity events too — drain until we see the Change.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("agent never received Change event before deadline");
        }
        let ev = tokio::time::timeout(remaining, adapter.next_event())
            .await
            .expect("agent timed out waiting for Change")
            .expect("pipeline closed");
        match ev {
            Event::Change { source, modality, from, to, .. } => {
                assert_eq!(source, "cpu");
                assert_eq!(modality, Modality::System);
                assert_eq!(from["cpu_pct"], 10.0);
                assert_eq!(to["cpu_pct"], 50.0);
                break;
            }
            Event::Entity { .. } => continue, // skip fusion noise
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

#[tokio::test]
async fn pipeline_e2e_snapshot_reflects_temporal_aggregate() {
    let ctx = PerceptionContext::start(PerceptionContextConfig::default());
    let adapter = ctx.new_adapter(Focus::default(), None, AdapterConfig::default());

    let (mock, tx) = MockPerceptionSource::new("cpu").with_streaming(64);
    ctx.raw_hub()
        .attach_source("cpu", Arc::new(mock) as Arc<dyn PerceptionSource>)
        .await;

    for v in [10.0, 20.0, 30.0_f64] {
        tx.send(raw_obs(
            "cpu",
            Modality::System,
            serde_json::json!({"cpu_pct": v}),
        ))
        .unwrap();
    }

    // Poll Snapshot::aggregates until the temporal processor has caught up.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let snap = adapter.now();
        if let Some(agg) = snap.aggregates.get(&("cpu".to_string(), Modality::System)) {
            if agg.stats["count"] == 3 {
                assert_eq!(agg.stats["min"], 10.0);
                assert_eq!(agg.stats["max"], 30.0);
                assert_eq!(agg.stats["mean"], 20.0);
                return;
            }
        }
    }
    panic!("temporal aggregate never reached count=3 in snapshot");
}

#[tokio::test]
async fn pipeline_e2e_summarize_uses_real_recent_events() {
    let summarizer: Arc<dyn PerceptionSummarizer> = Arc::new(EchoSummarizer);
    let ctx = PerceptionContext::start(PerceptionContextConfig::default());
    let adapter = ctx.new_adapter(
        Focus::default(),
        Some(summarizer),
        AdapterConfig::default(),
    );

    // Publish an anomaly directly to derived_hub — bypasses gating, lands in `recent`.
    ctx.derived_hub().publish(Event::Anomaly {
        source: "cpu".into(),
        reason: syscity::perception::AnomalyKind::SourceFault,
        severity: 200,
        at: SystemTime::now(),
    });

    // Allow the adapter forwarder to mirror it into recent_events.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let summary = adapter
        .summarize(Duration::from_secs(60))
        .await
        .expect("summary should succeed");
    assert!(summary.starts_with("echo:"), "got {summary}");
    // Make sure the user prompt actually carried payload (more than just an empty wrapper).
    let bytes: usize = summary["echo:".len()..]
        .split('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert!(bytes > 32, "summary prompt unexpectedly small: {summary}");
}

#[tokio::test]
async fn pipeline_e2e_two_agents_independent_focus() {
    let ctx = PerceptionContext::start(PerceptionContextConfig::default());

    // Agent A only cares about System; Agent B only cares about Audio.
    let a = ctx.new_adapter(
        Focus::default().with_modalities([Modality::System]),
        None,
        AdapterConfig::default(),
    );
    let b = ctx.new_adapter(
        Focus::default().with_modalities([Modality::Audio]),
        None,
        AdapterConfig::default(),
    );

    // Publish two Change events to derived_hub.
    let now = SystemTime::now();
    ctx.derived_hub().publish(Event::Change {
        source: "cpu".into(),
        modality: Modality::System,
        from: serde_json::json!(0),
        to: serde_json::json!(50),
        at: now,
    });
    ctx.derived_hub().publish(Event::Change {
        source: "mic".into(),
        modality: Modality::Audio,
        from: serde_json::json!(0),
        to: serde_json::json!(0.9),
        at: now,
    });

    let ev_a = tokio::time::timeout(Duration::from_secs(1), a.next_event())
        .await
        .expect("a timeout")
        .unwrap();
    let ev_b = tokio::time::timeout(Duration::from_secs(1), b.next_event())
        .await
        .expect("b timeout")
        .unwrap();

    // Each agent received exactly the modality in its focus.
    assert_eq!(ev_a.modality(), Some(Modality::System));
    assert_eq!(ev_b.modality(), Some(Modality::Audio));

    // No second event for either agent (other modality was gated out).
    let extra_a = tokio::time::timeout(Duration::from_millis(100), a.next_event()).await;
    let extra_b = tokio::time::timeout(Duration::from_millis(100), b.next_event()).await;
    assert!(extra_a.is_err(), "agent A should not receive Audio event");
    assert!(extra_b.is_err(), "agent B should not receive System event");
}

// ---------------------------------------------------------------------------
// ScreenshotAdapter Integration Tests
// ---------------------------------------------------------------------------

/// Stub ComputerAdapter that returns a controlled fake Screenshot (happy path).
struct StubScreenshotComputer {
    screenshot_value: Screenshot,
}

#[async_trait::async_trait]
impl ComputerAdapter for StubScreenshotComputer {
    async fn screenshot(&self, _region: Option<Rect>) -> syscity::computer::Result<Screenshot> {
        Ok(self.screenshot_value.clone())
    }

    async fn read_ui_tree(&self, _app: Option<&str>) -> syscity::computer::Result<Vec<UiElement>> {
        Ok(vec![])
    }

    async fn execute(&self, _action: DesktopAction) -> syscity::computer::Result<ActionResult> {
        Ok(ActionResult::success("stub"))
    }

    async fn wait_for(
        &self,
        _condition: WaitCondition,
        _timeout: Duration,
    ) -> syscity::computer::Result<bool> {
        Ok(true)
    }
}

#[tokio::test]
async fn screenshot_adapter_name_and_modality() {
    let headless = HeadlessComputerAdapter::new(Arc::new(ToolRegistry::new()));
    let adapter = ScreenshotAdapter::new(Arc::new(headless));
    assert_eq!(adapter.name(), "screenshot");
    assert_eq!(adapter.modality(), Modality::Rgb);
    assert!(adapter.status().is_healthy());
}

#[tokio::test]
async fn screenshot_adapter_observe_handles_no_display() {
    // HeadlessComputerAdapter without virtual display → ComputerError::NoDisplay
    // → ScreenshotAdapter swallows the error and returns empty Vec.
    let headless = Arc::new(HeadlessComputerAdapter::new(Arc::new(ToolRegistry::new())));
    let adapter = ScreenshotAdapter::new(headless);
    let obs = adapter.observe().await;
    assert!(obs.is_empty(), "expected empty observations when no display server");
}

#[tokio::test]
async fn screenshot_adapter_observe_happy_path() {
    let stub = Arc::new(StubScreenshotComputer {
        screenshot_value: Screenshot {
            base64: "dGVzdA==".to_string(),
            width: 1920,
            height: 1080,
            timestamp: Instant::now(),
        },
    });
    let adapter = ScreenshotAdapter::new(stub);
    let obs = adapter.observe().await;
    assert_eq!(obs.len(), 1, "expected 1 observation");
    assert_eq!(obs[0].source, "screenshot");
    assert_eq!(obs[0].modality, Modality::Rgb);
    assert_eq!(obs[0].confidence, 1.0);
    assert_eq!(obs[0].data["width"], 1920);
    assert_eq!(obs[0].data["height"], 1080);
    assert_eq!(obs[0].data["base64_length"], "dGVzdA==".len());
}

// ---------------------------------------------------------------------------
// SystemMonitorAdapter Integration Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_monitor_adapter_name_and_modality() {
    let monitor = Arc::new(Mutex::new(SystemMonitor::new()));
    let adapter = SystemMonitorAdapter::new(monitor);
    assert_eq!(adapter.name(), "system_monitor");
    assert_eq!(adapter.modality(), Modality::System);
    assert!(adapter.status().is_healthy());
}

#[tokio::test]
async fn system_monitor_adapter_observe_returns_status() {
    let monitor = Arc::new(Mutex::new(SystemMonitor::new()));
    let adapter = SystemMonitorAdapter::new(monitor);
    let obs = adapter.observe().await;
    assert_eq!(obs.len(), 1, "expected 1 observation");
    assert_eq!(obs[0].source, "system_monitor");
    assert_eq!(obs[0].modality, Modality::System);
    assert_eq!(obs[0].confidence, 1.0);

    // SystemStatus fields should be present in the observation data.
    let data = &obs[0].data;
    assert!(data.get("hostname").is_some(), "hostname should be present");
    assert!(
        data.get("cpu_usage_percent").is_some(),
        "cpu_usage_percent should be present"
    );
    assert!(
        data.get("memory_total_mb").is_some(),
        "memory_total_mb should be present"
    );
    assert!(
        data.get("os_name").is_some(),
        "os_name should be present"
    );
}

// ---------------------------------------------------------------------------
// Combined Registry Tests — ScreenshotAdapter + SystemMonitorAdapter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn screenshot_and_system_monitor_in_registry() {
    let reg = Arc::new(PerceptionRegistry::new(AggregationStrategy::Latest, 10));

    // Register ScreenshotAdapter with a stub ComputerAdapter that returns
    // a fake Screenshot (happy path — avoids needing a display server).
    let stub = Arc::new(StubScreenshotComputer {
        screenshot_value: Screenshot {
            base64: "dGVzdA==".to_string(),
            width: 1920,
            height: 1080,
            timestamp: Instant::now(),
        },
    });
    reg.register_source(Arc::new(ScreenshotAdapter::new(stub)))
        .await;

    // Register SystemMonitorAdapter.
    let monitor = Arc::new(Mutex::new(SystemMonitor::new()));
    reg.register_source(Arc::new(SystemMonitorAdapter::new(monitor)))
        .await;

    reg.poll_all().await;

    // Query all — both should appear.
    let result = reg.query(&PerceptionQuery::default()).await;
    let ids: Vec<String> = result.entities.iter().map(|e| e.id.to_string()).collect();
    assert!(
        ids.contains(&"screenshot".to_string()),
        "screenshot entity missing: {ids:?}"
    );
    assert!(
        ids.contains(&"system_monitor".to_string()),
        "system_monitor entity missing: {ids:?}"
    );

    // Filter by modality: Rgb → only screenshot.
    let q = PerceptionQuery { modalities: Some(vec![Modality::Rgb]), ..Default::default() };
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 1, "expected 1 Rgb entity");
    assert_eq!(result.entities[0].id.to_string(), "screenshot");
    // Screenshot properties contain the observation data under the "data" key.
    let ss_data = &result.entities[0].properties["data"];
    assert_eq!(ss_data["width"].as_u64(), Some(1920));
    assert_eq!(ss_data["height"].as_u64(), Some(1080));

    // Filter by modality: System → only system_monitor.
    let q = PerceptionQuery { modalities: Some(vec![Modality::System]), ..Default::default() };
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 1, "expected 1 System entity");
    assert_eq!(result.entities[0].id.to_string(), "system_monitor");
    // SystemMonitor properties under "data" should have hostname.
    let sm_data = &result.entities[0].properties["data"];
    assert!(sm_data.get("hostname").is_some(), "hostname should be present");
}

