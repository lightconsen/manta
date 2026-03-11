//! Memory tool for Manta
//!
//! This tool allows the AI to store and retrieve memories (facts, preferences,
//! context) that persist across conversations.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};

/// Memory tool for storing and retrieving information
#[derive(Debug)]
pub struct MemoryTool {
    /// Storage backend
    storage: MemoryStorage,
}

/// In-memory storage for quick access
#[derive(Debug, Clone)]
struct MemoryStorage {
    memories: std::sync::Arc<tokio::sync::RwLock<Vec<MemoryEntry>>>,
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self {
            memories: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        }
    }
}

impl MemoryStorage {
    async fn store(&self, entry: MemoryEntry) -> crate::Result<String> {
        let mut memories = self.memories.write().await;
        let id = format!("mem_{}", memories.len() + 1);
        let entry = MemoryEntry { id: id.clone(), ..entry };
        memories.push(entry);
        Ok(id)
    }

    async fn retrieve(&self, id: &str) -> Option<MemoryEntry> {
        let memories = self.memories.read().await;
        memories.iter().find(|m| m.id == id).cloned()
    }

    async fn search(&self, query: &str, category: Option<&str>) -> Vec<MemoryEntry> {
        let memories = self.memories.read().await;
        let query_lower = query.to_lowercase();

        memories
            .iter()
            .filter(|m| {
                // Filter by category if specified
                if let Some(cat) = category {
                    if m.category.as_deref() != Some(cat) {
                        return false;
                    }
                }

                // Search in content, key, category, and tags
                m.content.to_lowercase().contains(&query_lower) ||
                m.key.as_ref().map(|k| k.to_lowercase().contains(&query_lower)).unwrap_or(false) ||
                m.category.as_ref().map(|c| c.to_lowercase().contains(&query_lower)).unwrap_or(false) ||
                m.tags.iter().any(|t| t.to_lowercase().contains(&query_lower))
            })
            .cloned()
            .collect()
    }

    async fn list(&self, category: Option<&str>, limit: usize) -> Vec<MemoryEntry> {
        let memories = self.memories.read().await;

        memories
            .iter()
            .filter(|m| {
                if let Some(cat) = category {
                    m.category.as_deref() == Some(cat)
                } else {
                    true
                }
            })
            .take(limit)
            .cloned()
            .collect()
    }

    async fn delete(&self, id: &str) -> bool {
        let mut memories = self.memories.write().await;
        let initial_len = memories.len();
        memories.retain(|m| m.id != id);
        memories.len() < initial_len
    }

    async fn update(&self, id: &str, updates: MemoryUpdate) -> crate::Result<bool> {
        let mut memories = self.memories.write().await;

        if let Some(memory) = memories.iter_mut().find(|m| m.id == id) {
            if let Some(content) = updates.content {
                memory.content = content;
            }
            if let Some(category) = updates.category {
                memory.category = Some(category);
            }
            if let Some(key) = updates.key {
                memory.key = Some(key);
            }
            if let Some(tags) = updates.tags {
                memory.tags = tags;
            }
            memory.updated_at = chrono::Utc::now();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// A memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryEntry {
    id: String,
    content: String,
    key: Option<String>,
    category: Option<String>,
    tags: Vec<String>,
    importance: i32,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    user_id: Option<String>,
    conversation_id: Option<String>,
}

/// Updates for a memory entry
#[derive(Debug, Clone)]
struct MemoryUpdate {
    content: Option<String>,
    category: Option<String>,
    key: Option<String>,
    tags: Option<Vec<String>>,
}

impl Default for MemoryTool {
    fn default() -> Self {
        Self {
            storage: MemoryStorage::default(),
        }
    }
}

impl MemoryTool {
    /// Create a new memory tool
    pub fn new() -> Self {
        Self::default()
    }

    /// Extract keywords from content for tagging
    fn extract_keywords(&self, content: &str) -> Vec<String> {
        let stop_words = ["the", "a", "an", "is", "are", "was", "were", "be", "been",
                         "being", "have", "has", "had", "do", "does", "did", "will",
                         "would", "could", "should", "may", "might", "must", "shall",
                         "can", "need", "dare", "ought", "used", "to", "of", "in",
                         "for", "on", "with", "at", "by", "from", "as", "into",
                         "through", "during", "before", "after", "above", "below",
                         "between", "under", "over", "and", "but", "or", "yet", "so", "if",
                         "because", "although", "though", "while", "where", "when",
                         "that", "which", "who", "whom", "whose", "what", "this",
                         "these", "those", "i", "you", "he", "she", "it", "we",
                         "they", "me", "him", "her", "us", "them", "my", "your",
                         "his", "hers", "its", "our", "their"];

        content
            .to_lowercase()
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .filter(|w| w.len() > 3 && !stop_words.contains(w))
            .map(|w| w.to_string())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .take(5)
            .collect()
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        r#"Store and retrieve memories, facts, and context that persist across conversations.

Use this to:
- Remember important information about the user
- Store facts for later retrieval
- Keep track of preferences
- Save context from previous conversations
- Build a knowledge base over time

Categories: 'user' (user preferences), 'fact' (general facts), 'context' (conversation context),
'task' (task-related), 'project' (project-specific)"#
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Memory operations",
            serde_json::json!({
                "action": {
                    "type": "string",
                    "enum": ["store", "retrieve", "search", "list", "delete", "update"],
                    "description": "The memory operation to perform"
                },
                "content": {
                    "type": "string",
                    "description": "Content to store (for 'store' action)"
                },
                "id": {
                    "type": "string",
                    "description": "Memory ID (for 'retrieve', 'delete', 'update')"
                },
                "key": {
                    "type": "string",
                    "description": "Unique key for the memory (optional)"
                },
                "category": {
                    "type": "string",
                    "description": "Category for the memory (user, fact, context, task, project)"
                },
                "tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Tags for the memory"
                },
                "query": {
                    "type": "string",
                    "description": "Search query (for 'search' action)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results",
                    "default": 10
                },
                "importance": {
                    "type": "integer",
                    "description": "Importance level 1-5 (default: 3)",
                    "minimum": 1,
                    "maximum": 5,
                    "default": 3
                }
            }),
            vec!["action"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| crate::error::MantaError::Validation("Missing 'action' argument".to_string()))?;

        match action {
            "store" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'content' argument".to_string()))?;

                let key = args["key"].as_str().map(|s| s.to_string());
                let category = args["category"].as_str().map(|s| s.to_string());

                let tags: Vec<String> = args["tags"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_else(|| self.extract_keywords(content));

                let importance = args["importance"].as_i64().map(|i| i as i32).unwrap_or(3);

                let entry = MemoryEntry {
                    id: String::new(), // Will be assigned by storage
                    content: content.to_string(),
                    key,
                    category,
                    tags,
                    importance,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    user_id: Some(context.user_id.clone()),
                    conversation_id: Some(context.conversation_id.clone()),
                };

                let id = self.storage.store(entry).await?;
                info!("Stored memory with ID: {}", id);

                Ok(ToolExecutionResult::success(format!("Memory stored with ID: {}", id))
                    .with_data(serde_json::json!({"id": id, "stored": true})))
            }

            "retrieve" => {
                let id = args["id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'id' argument".to_string()))?;

                match self.storage.retrieve(id).await {
                    Some(memory) => {
                        debug!("Retrieved memory: {}", id);
                        Ok(ToolExecutionResult::success(memory.content.clone())
                            .with_data(serde_json::json!(memory)))
                    }
                    None => Ok(ToolExecutionResult::error(format!("Memory '{}' not found", id))),
                }
            }

            "search" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'query' argument".to_string()))?;

                let category = args["category"].as_str();
                let limit = args["limit"].as_u64().map(|l| l as usize).unwrap_or(10);

                let results = self.storage.search(query, category).await;
                let results: Vec<_> = results.into_iter().take(limit).collect();

                if results.is_empty() {
                    return Ok(ToolExecutionResult::success(format!(
                        "No memories found matching '{}'",
                        query
                    )));
                }

                let formatted: Vec<String> = results
                    .iter()
                    .map(|m| {
                        let tags = if m.tags.is_empty() {
                            String::new()
                        } else {
                            format!(" [tags: {}]", m.tags.join(", "))
                        };
                        format!("[{}] {}{}", m.id, m.content, tags)
                    })
                    .collect();

                info!("Found {} memories matching '{}'", results.len(), query);

                Ok(ToolExecutionResult::success(formatted.join("\n\n"))
                    .with_data(serde_json::json!({
                        "count": results.len(),
                        "memories": results
                    })))
            }

            "list" => {
                let category = args["category"].as_str();
                let limit = args["limit"].as_u64().map(|l| l as usize).unwrap_or(10);

                let memories = self.storage.list(category, limit).await;

                if memories.is_empty() {
                    let cat_msg = category.map(|c| format!(" in category '{}'", c)).unwrap_or_default();
                    return Ok(ToolExecutionResult::success(format!(
                        "No memories found{}",
                        cat_msg
                    )));
                }

                let formatted: Vec<String> = memories
                    .iter()
                    .map(|m| {
                        let cat = m.category.as_deref().unwrap_or("uncategorized");
                        format!("[{}] ({}): {}", m.id, cat, m.content.chars().take(100).collect::<String>())
                    })
                    .collect();

                Ok(ToolExecutionResult::success(formatted.join("\n"))
                    .with_data(serde_json::json!({
                        "count": memories.len(),
                        "memories": memories
                    })))
            }

            "delete" => {
                let id = args["id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'id' argument".to_string()))?;

                if self.storage.delete(id).await {
                    info!("Deleted memory: {}", id);
                    Ok(ToolExecutionResult::success(format!("Memory '{}' deleted", id)))
                } else {
                    Ok(ToolExecutionResult::error(format!("Memory '{}' not found", id)))
                }
            }

            "update" => {
                let id = args["id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'id' argument".to_string()))?;

                let updates = MemoryUpdate {
                    content: args["content"].as_str().map(|s| s.to_string()),
                    category: args["category"].as_str().map(|s| s.to_string()),
                    key: args["key"].as_str().map(|s| s.to_string()),
                    tags: args["tags"].as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    }),
                };

                match self.storage.update(id, updates).await? {
                    true => {
                        info!("Updated memory: {}", id);
                        Ok(ToolExecutionResult::success(format!("Memory '{}' updated", id)))
                    }
                    false => Ok(ToolExecutionResult::error(format!("Memory '{}' not found", id))),
                }
            }

            _ => Err(crate::error::MantaError::Validation(format!(
                "Unknown action: {}. Valid actions: store, retrieve, search, list, delete, update",
                action
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_tool_creation() {
        let tool = MemoryTool::new();
        assert_eq!(tool.name(), "memory");
    }

    #[tokio::test]
    async fn test_memory_store_and_retrieve() {
        let tool = MemoryTool::new();
        let context = ToolContext::new("user1", "conv1");

        // Store a memory
        let store_args = serde_json::json!({
            "action": "store",
            "content": "User prefers dark mode",
            "category": "user",
            "key": "dark_mode_preference"
        });

        let result = tool.execute(store_args, &context).await.unwrap();
        assert!(result.success);

        let id = result.data.unwrap()["id"].as_str().unwrap().to_string();

        // Retrieve the memory
        let retrieve_args = serde_json::json!({
            "action": "retrieve",
            "id": id
        });

        let result = tool.execute(retrieve_args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("dark mode"));
    }

    #[tokio::test]
    async fn test_memory_search() {
        let tool = MemoryTool::new();
        let context = ToolContext::new("user1", "conv1");

        // Store some memories
        let memories = vec![
            ("User likes pizza", "food"),
            ("User is vegetarian", "food"),
            ("User works remotely", "work"),
        ];

        for (content, cat) in memories {
            let args = serde_json::json!({
                "action": "store",
                "content": content,
                "category": cat
            });
            tool.execute(args, &context).await.unwrap();
        }

        // Search for food-related memories
        let search_args = serde_json::json!({
            "action": "search",
            "query": "food"
        });

        let result = tool.execute(search_args, &context).await.unwrap();
        assert!(result.success);
        // Should find at least the vegetarian memory
        assert!(result.output.contains("vegetarian") || result.output.contains("pizza"));
    }

    #[test]
    fn test_extract_keywords() {
        let tool = MemoryTool::new();
        let keywords = tool.extract_keywords("The quick brown fox jumps over the lazy dog");

        // Should extract meaningful words, excluding stop words
        assert!(!keywords.is_empty());
        assert!(!keywords.contains(&"the".to_string()));
        assert!(!keywords.contains(&"over".to_string()));
    }
}
