//! Knowledge graph node/edge types and heuristic entity extraction.

use super::*;

/// Lightweight knowledge graph node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    /// Entity label.
    pub label: String,
    /// Entity type (person, place, concept, etc.).
    pub node_type: String,
    /// Related memory IDs.
    pub memory_ids: Vec<String>,
    /// Confidence score.
    pub confidence: f32,
}

/// Lightweight knowledge graph edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEdge {
    /// Source node label.
    pub from: String,
    /// Target node label.
    pub to: String,
    /// Relationship type.
    pub relation: String,
    /// Confidence score.
    pub confidence: f32,
}

/// In-memory knowledge graph (REM phase output).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeGraph {
    pub nodes: Vec<KnowledgeNode>,
    pub edges: Vec<KnowledgeEdge>,
}

impl KnowledgeGraph {
    /// Save the knowledge graph to disk as JSON.
    pub async fn save_to_disk(&self, path: impl AsRef<std::path::Path>) -> crate::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            crate::error::SyscityError::Storage {
                context: "Failed to serialize knowledge graph".to_string(),
                details: e.to_string(),
            }
        })?;
        if let Some(parent) = path.as_ref().parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                crate::error::SyscityError::Storage {
                    context: format!("Failed to create knowledge graph directory: {:?}", parent),
                    details: e.to_string(),
                }
            })?;
        }
        tokio::fs::write(path, json)
            .await
            .map_err(|e| crate::error::SyscityError::Storage {
                context: "Failed to write knowledge graph".to_string(),
                details: e.to_string(),
            })?;
        Ok(())
    }

    /// Load the knowledge graph from disk.
    pub async fn load_from_disk(path: impl AsRef<std::path::Path>) -> crate::Result<Self> {
        let json = tokio::fs::read_to_string(path).await.map_err(|e| {
            crate::error::SyscityError::Storage {
                context: "Failed to read knowledge graph".to_string(),
                details: e.to_string(),
            }
        })?;
        serde_json::from_str(&json).map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to deserialize knowledge graph".to_string(),
            details: e.to_string(),
        })
    }

    /// Maximum number of nodes before eviction kicks in.
    const MAX_NODES: usize = 10_000;
    /// Maximum number of edges before eviction kicks in.
    const MAX_EDGES: usize = 50_000;

    /// Cap the graph size by evicting lowest-confidence entries when limits
    /// are exceeded.  This prevents unbounded memory growth across REM cycles.
    ///
    /// NaN confidence values are replaced with 0.0 so they are evicted first
    /// rather than making sort order non-deterministic.
    pub fn cap_size(&mut self) {
        // Normalize NaN confidence values before sorting.
        for node in &mut self.nodes {
            if node.confidence.is_nan() {
                node.confidence = 0.0;
            }
        }
        for edge in &mut self.edges {
            if edge.confidence.is_nan() {
                edge.confidence = 0.0;
            }
        }

        if self.nodes.len() > Self::MAX_NODES {
            self.nodes.sort_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.nodes.truncate(Self::MAX_NODES);
            // Re-sort by confidence descending for normal usage.
            self.nodes.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        if self.edges.len() > Self::MAX_EDGES {
            self.edges.sort_by(|a, b| {
                a.confidence
                    .partial_cmp(&b.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.edges.truncate(Self::MAX_EDGES);
            // Re-sort by confidence descending for normal usage.
            self.edges.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
}

/// Heuristic entity extraction fallback when LLM is unavailable.
/// Extracts capitalized words appearing 3+ times as entities.
pub(super) fn extract_entities_heuristic(
    memories: &[Memory],
) -> (Vec<KnowledgeNode>, Vec<KnowledgeEdge>) {
    let mut word_counts: HashMap<String, u32> = HashMap::new();
    let processed = memories.len() as u32;

    for mem in memories {
        for word in mem.content.split_whitespace() {
            let clean = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string();
            if clean.len() > 3
                && clean
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
            {
                *word_counts.entry(clean.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for (word, count) in &word_counts {
        if *count >= 3 {
            let node = KnowledgeNode {
                label: word.clone(),
                node_type: "concept".to_string(),
                memory_ids: memories
                    .iter()
                    .filter(|m| {
                        let pattern = word.to_lowercase();
                        m.content.to_lowercase().split_whitespace().any(|w| {
                            w.trim_matches(|c: char| !c.is_alphanumeric()) == pattern.as_str()
                        })
                    })
                    .map(|m| m.id.to_string())
                    .collect(),
                confidence: (*count as f32 / processed as f32).min(1.0),
            };
            nodes.push(node);
        }
    }

    for (i, n1) in nodes.iter().enumerate() {
        for n2 in nodes.iter().skip(i + 1) {
            let shared: Vec<_> = n1
                .memory_ids
                .iter()
                .filter(|id| n2.memory_ids.contains(id))
                .collect();
            if !shared.is_empty() {
                edges.push(KnowledgeEdge {
                    from: n1.label.clone(),
                    to: n2.label.clone(),
                    relation: "co_occurs".to_string(),
                    confidence: (shared.len() as f32
                        / n1.memory_ids.len().max(n2.memory_ids.len()) as f32)
                        .min(1.0),
                });
            }
        }
    }

    (nodes, edges)
}
