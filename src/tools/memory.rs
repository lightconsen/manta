//! Memory tool for Manta
//!
//! This tool allows the AI to store and retrieve memories (facts, preferences,
//! context) that persist across conversations using SQLite storage.

use super::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, info};

use crate::memory::{Memory, MemoryQuery, SqliteMemoryStore, MemoryId, MemoryStore};

/// Memory tool for storing and retrieving information
#[derive(Debug, Clone)]
pub struct MemoryTool {
    /// SQLite storage backend
    storage: Arc<SqliteMemoryStore>,
}

impl MemoryTool {
    /// Create a new memory tool with SQLite storage
    pub async fn new() -> crate::Result<Self> {
        // Use centralized ~/.manta/memory directory
        let db_path = crate::dirs::default_memory_db();
        let db_url = format!("sqlite:{}", db_path.display());

        info!("Initializing memory tool with database: {}", db_path.display());

        let storage = Arc::new(SqliteMemoryStore::new(&db_url).await?);

        Ok(Self { storage })
    }

    /// Create with custom database URL (for testing)
    pub async fn with_database_url(database_url: &str) -> crate::Result<Self> {
        let storage = Arc::new(SqliteMemoryStore::new(database_url).await?);
        Ok(Self { storage })
    }

    /// Create with an existing store (for sharing with agent)
    pub async fn with_store(storage: Arc<SqliteMemoryStore>) -> crate::Result<Self> {
        Ok(Self { storage })
    }

    /// Search for relevant memories to inject into context
    pub async fn search_relevant(&self, query: &str, user_id: &str, limit: usize) -> crate::Result<Vec<Memory>> {
        let memory_query = MemoryQuery::new()
            .for_user(user_id)
            .with_content(query)
            .limit(limit);

        let results = self.storage.search(memory_query).await?;
        Ok(results)
    }

    /// Get recent memories for a user
    pub async fn get_recent(&self, user_id: &str, limit: usize) -> crate::Result<Vec<Memory>> {
        let memory_query = MemoryQuery::new()
            .for_user(user_id)
            .limit(limit);

        let results = self.storage.search(memory_query).await?;
        Ok(results)
    }

    /// Format memories for injection into system prompt
    pub fn format_memories_for_prompt(memories: &[Memory]) -> String {
        if memories.is_empty() {
            return String::new();
        }

        let mut sections = vec!["### Relevant Memories".to_string()];

        for mem in memories.iter().take(5) {
            let mem_type = &mem.memory_type;
            let content = &mem.content;
            sections.push(format!("- [{}] {}", mem_type, content));
        }

        sections.join("\n")
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        r#"Store and retrieve memories, facts, and context that persist across conversations.

This tool saves information to a SQLite database that persists across restarts.
Use this to:
- Remember important information about the user
- Store facts for later retrieval
- Keep track of preferences
- Save context from previous conversations
- Build a knowledge base over time

Categories: 'user' (user preferences), 'fact' (general facts), 'context' (conversation context),
'task' (task-related), 'project' (project-specific)

Memories are automatically searched and relevant ones injected into new conversations."#
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
                "category": {
                    "type": "string",
                    "description": "Category for the memory (user, fact, context, task, project)"
                },
                "query": {
                    "type": "string",
                    "description": "Search query (for 'search' action)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results",
                    "default": 10
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

                let memory_type = args["category"]
                    .as_str()
                    .unwrap_or("fact");

                let memory = Memory::new(
                    context.user_id.clone(),
                    content.to_string(),
                    memory_type.to_string(),
                ).with_conversation(context.conversation_id.clone());

                let memory_id = self.storage.store(memory).await?;
                info!("Stored memory with ID: {}", memory_id);

                Ok(ToolExecutionResult::success(format!("Memory stored with ID: {}", memory_id.0))
                    .with_data(serde_json::json!({"id": memory_id.0, "stored": true})))
            }

            "retrieve" => {
                let id = args["id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'id' argument".to_string()))?;

                let memory_id = MemoryId::new(id);
                match self.storage.get(&memory_id).await? {
                    Some(memory) => {
                        debug!("Retrieved memory: {}", id);
                        Ok(ToolExecutionResult::success(memory.content.clone())
                            .with_data(serde_json::json!({
                                "id": memory.id.0,
                                "content": memory.content,
                                "memory_type": memory.memory_type,
                                "created_at": memory.created_at
                            })))
                    }
                    None => Ok(ToolExecutionResult::error(format!("Memory '{}' not found", id))),
                }
            }

            "search" => {
                let query = args["query"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'query' argument".to_string()))?;

                let limit = args["limit"].as_u64().map(|l| l as usize).unwrap_or(10);
                let category = args["category"].as_str();

                let mut memory_query = MemoryQuery::new()
                    .for_user(&context.user_id)
                    .with_content(query)
                    .limit(limit);

                if let Some(cat) = category {
                    memory_query = memory_query.of_type(cat);
                }

                let results = self.storage.search(memory_query).await?;

                if results.is_empty() {
                    return Ok(ToolExecutionResult::success(format!(
                        "No memories found matching '{}'",
                        query
                    )));
                }

                let formatted: Vec<String> = results
                    .iter()
                    .map(|m| {
                        format!("[{}] ({}): {}", m.id.0, m.memory_type, m.content)
                    })
                    .collect();

                info!("Found {} memories matching '{}'", results.len(), query);

                Ok(ToolExecutionResult::success(formatted.join("\n\n"))
                    .with_data(serde_json::json!({
                        "count": results.len(),
                        "memories": results.iter().map(|m| serde_json::json!({
                            "id": m.id.0,
                            "content": m.content,
                            "memory_type": m.memory_type
                        })).collect::<Vec<_>>()
                    })))
            }

            "list" => {
                let limit = args["limit"].as_u64().map(|l| l as usize).unwrap_or(10);
                let category = args["category"].as_str();

                let mut memory_query = MemoryQuery::new()
                    .for_user(&context.user_id)
                    .limit(limit);

                if let Some(cat) = category {
                    memory_query = memory_query.of_type(cat);
                }

                let memories = self.storage.search(memory_query).await?;

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
                        format!("[{}] ({}): {}", m.id.0, m.memory_type, m.content.chars().take(100).collect::<String>())
                    })
                    .collect();

                Ok(ToolExecutionResult::success(formatted.join("\n"))
                    .with_data(serde_json::json!({
                        "count": memories.len(),
                        "memories": memories.iter().map(|m| serde_json::json!({
                            "id": m.id.0,
                            "content": m.content,
                            "memory_type": m.memory_type
                        })).collect::<Vec<_>>()
                    })))
            }

            "delete" => {
                let id = args["id"]
                    .as_str()
                    .ok_or_else(|| crate::error::MantaError::Validation("Missing 'id' argument".to_string()))?;

                let memory_id = MemoryId::new(id);
                if self.storage.delete(&memory_id).await? {
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

                let memory_id = MemoryId::new(id);

                // Get existing memory
                let mut memory = match self.storage.get(&memory_id).await? {
                    Some(m) => m,
                    None => return Ok(ToolExecutionResult::error(format!("Memory '{}' not found", id))),
                };

                // Update fields if provided
                if let Some(content) = args["content"].as_str() {
                    memory.content = content.to_string();
                }
                if let Some(memory_type) = args["category"].as_str() {
                    memory.memory_type = memory_type.to_string();
                }

                self.storage.update(memory).await?;
                info!("Updated memory: {}", id);

                Ok(ToolExecutionResult::success(format!("Memory '{}' updated", id)))
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

    #[tokio::test]
    async fn test_memory_tool_creation() {
        let tool = MemoryTool::with_database_url("sqlite::memory:").await.unwrap();
        assert_eq!(tool.name(), "memory");
    }

    #[tokio::test]
    async fn test_memory_store_and_retrieve() {
        let tool = MemoryTool::with_database_url("sqlite::memory:").await.unwrap();
        let context = ToolContext::new("user1", "conv1");

        // Store a memory
        let store_args = serde_json::json!({
            "action": "store",
            "content": "User prefers dark mode",
            "category": "user"
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
        let tool = MemoryTool::with_database_url("sqlite::memory:").await.unwrap();
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
    }
}
