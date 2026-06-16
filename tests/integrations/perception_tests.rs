//! Integration tests for the perception fusion layer.
//!
//! These tests validate the full flow: register mock perception sources,
//! poll observations, query entities by modality/source, and exercise the
//! PerceptionQueryTool.

use std::sync::Arc;

use syscity::perception::mock::MockPerceptionSource;
use syscity::perception::{
    AggregationStrategy, Modality, PerceptionQuery, PerceptionQueryTool, PerceptionRegistry,
};
use syscity::tools::{Tool, ToolContext};

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

    let mut q = PerceptionQuery::default();
    q.modalities = Some(vec![Modality::System]);
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 1, "expected 1 system entity");
    assert_eq!(result.entities[0].modality, Modality::System);
}

#[tokio::test]
async fn perception_query_by_source() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let mut q = PerceptionQuery::default();
    q.sources = Some(vec!["camera".to_string()]);
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

    let mut q = PerceptionQuery::default();
    q.label_contains = Some("Rgb".to_string());
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 1, "expected 1 entity with label containing 'Rgb'");
}

#[tokio::test]
async fn perception_query_with_limit() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let mut q = PerceptionQuery::default();
    q.limit = Some(2);
    let result = reg.query(&q).await;
    assert_eq!(result.entities.len(), 2, "expected 2 entities (limited)");
}

#[tokio::test]
async fn perception_query_no_matches() {
    let reg = setup_registry().await;
    reg.poll_all().await;

    let mut q = PerceptionQuery::default();
    q.modalities = Some(vec![Modality::Audio]);
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
