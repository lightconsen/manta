//! Perception query tool.
//!
//! Registers a [`perception_query`](PerceptionQueryTool) tool that allows the
//! LLM to query the current perceptual state of the world through the
//! [`PerceptionRegistry`].

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::perception::{FusionConfig, FusionEngine, PerceptionRegistry};
use crate::tools::{Tool, ToolCapabilities, ToolContext, ToolExecutionResult};

/// Tool that queries the perception fusion layer.
///
/// Calling this tool triggers a poll of all registered perception sources,
/// ingests observations into the scene graph, and returns matching entities.
/// When [`FusionEngine`] is configured, results include cross-modal fused entities.
pub struct PerceptionQueryTool {
    registry: Arc<PerceptionRegistry>,
    fusion_engine: Option<FusionEngine>,
}

impl PerceptionQueryTool {
    /// Create a new perception query tool.
    pub fn new(registry: Arc<PerceptionRegistry>) -> Self {
        Self {
            registry,
            fusion_engine: None,
        }
    }

    /// Enable cross-modal fusion with the given configuration.
    ///
    /// When set, the tool output will include `fused_entities` derived from
    /// temporal and modality-based correlation of observations.
    pub fn with_fusion(mut self, config: FusionConfig) -> Self {
        self.fusion_engine = Some(FusionEngine::new(config));
        self
    }
}

#[async_trait]
impl Tool for PerceptionQueryTool {
    fn name(&self) -> &str {
        "perception_query"
    }

    fn description(&self) -> &str {
        "Query the current perceptual state of the world. Polls all sensors \
         (screenshots, system monitors, device sensors) and returns structured \
         observations and entities matching your query. Use this to answer \
         questions like \"what's the current system state?\" or \
         \"what sensors are available?\""
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "modalities": {
                    "type": "array",
                    "items": {"type": "string", "enum": ["rgb", "depth", "audio", "tactile", "system", "device", "ui_tree", "file_system", "network"]},
                    "description": "Filter by sensor modalities"
                },
                "sources": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Filter by source name (e.g. \"screenshot\", \"system_monitor\", \"device:sensor-01:temperature\")"
                },
                "label_contains": {
                    "type": "string",
                    "description": "Substring match on entity labels"
                },
                "min_confidence": {
                    "type": "number",
                    "description": "Minimum confidence threshold [0.0, 1.0]"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of entities to return"
                },
                "enable_fusion": {
                    "type": "boolean",
                    "description": "When true, run cross-modal fusion on observations and include fused_entities in output"
                }
            }
        })
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let query = parse_query(&args);
        let enable_fusion = args
            .get("enable_fusion")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Poll all sources and query
        self.registry.poll_all().await;
        let result = self.registry.query(&query).await;

        let mut output = serde_json::json!({
            "entities": result.entities,
            "sources": self.registry.list_sources().await,
            "source_statuses": self.registry.list_source_statuses().await,
        });

        // Post-process through FusionEngine if enabled
        if enable_fusion {
            if let Some(engine) = &self.fusion_engine {
                let observations = self.registry.all_observations().await;
                let fused = engine.fuse(&observations);
                output["fused_entities"] =
                    serde_json::to_value(&fused).unwrap_or(serde_json::Value::Null);
            }
        }

        Ok(ToolExecutionResult::success(
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "{}".to_string()),
        ))
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            ..Default::default()
        }
    }
}

/// Parse a [`crate::perception::PerceptionQuery`] from JSON arguments.
fn parse_query(args: &Value) -> crate::perception::PerceptionQuery {
    use crate::perception::PerceptionQuery;

    let mut q = PerceptionQuery::default();

    if let Some(mods) = args.get("modalities").and_then(|v| v.as_array()) {
        let parsed: Vec<crate::perception::Modality> = mods
            .iter()
            .filter_map(|v| v.as_str())
            .filter_map(|s| match s {
                "rgb" => Some(crate::perception::Modality::Rgb),
                "depth" => Some(crate::perception::Modality::Depth),
                "audio" => Some(crate::perception::Modality::Audio),
                "tactile" => Some(crate::perception::Modality::Tactile),
                "system" => Some(crate::perception::Modality::System),
                "device" => Some(crate::perception::Modality::Device),
                "ui_tree" => Some(crate::perception::Modality::UiTree),
                "file_system" => Some(crate::perception::Modality::FileSystem),
                "network" => Some(crate::perception::Modality::Network),
                _ => None,
            })
            .collect();
        if !parsed.is_empty() {
            q.modalities = Some(parsed);
        }
    }

    if let Some(sources) = args.get("sources").and_then(|v| v.as_array()) {
        let parsed: Vec<String> = sources
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        if !parsed.is_empty() {
            q.sources = Some(parsed);
        }
    }

    if let Some(label) = args.get("label_contains").and_then(|v| v.as_str()) {
        if !label.is_empty() {
            q.label_contains = Some(label.to_string());
        }
    }

    if let Some(conf) = args.get("min_confidence").and_then(|v| v.as_f64()) {
        q.min_confidence = Some(conf as f32);
    }

    if let Some(limit) = args.get("limit").and_then(|v| v.as_u64()) {
        q.limit = Some(limit as usize);
    }

    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query_empty() {
        let q = parse_query(&serde_json::json!({}));
        assert!(q.modalities.is_none());
        assert!(q.sources.is_none());
    }

    #[test]
    fn test_parse_query_with_modalities() {
        let q = parse_query(&serde_json::json!({
            "modalities": ["rgb", "system"]
        }));
        assert_eq!(q.modalities.unwrap().len(), 2);
    }

    #[test]
    fn test_parse_query_with_sources() {
        let q = parse_query(&serde_json::json!({
            "sources": ["screenshot", "system_monitor"]
        }));
        assert_eq!(q.sources.unwrap().len(), 2);
    }

    #[test]
    fn test_tool_name() {
        let reg = Arc::new(crate::perception::PerceptionRegistry::new(
            crate::perception::AggregationStrategy::Latest,
            10,
        ));
        let tool = PerceptionQueryTool::new(reg);
        assert_eq!(tool.name(), "perception_query");
        assert!(!tool.capabilities().requires_approval);
    }
}
