//! Persistent Assistant Spawning System
//!
//! This module allows Manta to create and manage other specialized Personal AI Assistants,
//! each with their own identity, memory, and capabilities.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};
use uuid::Uuid;

// Re-export process types
pub use process::{AssistantProcess, IpcMessage, ProcessManager};

/// Type of specialized assistant
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantType {
    /// Deep research and analysis
    Researcher,
    /// Code review and PR analysis
    CodeReviewer,
    /// Calendar, reminders, time management
    Scheduler,
    /// Different persona/tone for social channels
    Social,
    /// Custom specialization
    Specialist(String),
}

impl AssistantType {
    /// Get default system prompt for this type
    pub fn default_system_prompt(&self) -> String {
        match self {
            AssistantType::Researcher => {
                "You are a deep research assistant. Your job is to thoroughly research topics, \
                analyze information from multiple sources, and provide comprehensive summaries. \
                You have access to web search and can fetch content from URLs. \
                Always cite your sources and provide structured, detailed responses."
                    .to_string()
            }
            AssistantType::CodeReviewer => {
                "You are a code review assistant. Your job is to analyze code for bugs, \
                security issues, performance problems, and style violations. \
                You have access to file reading tools and can search codebases. \
                Provide constructive feedback with specific line references."
                    .to_string()
            }
            AssistantType::Scheduler => {
                "You are a scheduling assistant. Your job is to manage calendars, \
                set reminders, and help with time management. \
                You have access to cron scheduling and time utilities. \
                Be precise with dates and times, and confirm all scheduling actions."
                    .to_string()
            }
            AssistantType::Social => {
                "You are a social media assistant with a friendly, engaging personality. \
                Your job is to help draft posts, respond to messages, and manage social interactions. \
                Adapt your tone to the platform and audience."
                    .to_string()
            }
            AssistantType::Specialist(name) => {
                format!(
                    "You are a specialized assistant focused on {}. \
                    Provide expert-level assistance in your domain.",
                    name
                )
            }
        }
    }

    /// Get default tools for this type
    pub fn default_tools(&self) -> Vec<String> {
        match self {
            AssistantType::Researcher => {
                vec![
                    "web_search".to_string(),
                    "web_fetch".to_string(),
                    "memory".to_string(),
                ]
            }
            AssistantType::CodeReviewer => {
                vec![
                    "file_read".to_string(),
                    "glob".to_string(),
                    "shell".to_string(),
                    "memory".to_string(),
                ]
            }
            AssistantType::Scheduler => {
                vec!["cron".to_string(), "time".to_string(), "memory".to_string()]
            }
            AssistantType::Social => {
                vec!["memory".to_string(), "web_fetch".to_string()]
            }
            AssistantType::Specialist(_) => {
                vec!["memory".to_string()]
            }
        }
    }
}

impl std::fmt::Display for AssistantType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssistantType::Researcher => write!(f, "researcher"),
            AssistantType::CodeReviewer => write!(f, "code_reviewer"),
            AssistantType::Scheduler => write!(f, "scheduler"),
            AssistantType::Social => write!(f, "social"),
            AssistantType::Specialist(name) => write!(f, "specialist:{}", name),
        }
    }
}

/// Configuration for a channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    /// Channel type
    pub channel_type: String,
    /// Channel identifier (e.g., "@bot_name" for Telegram)
    pub identifier: String,
    /// Whether this channel is enabled
    pub enabled: bool,
    /// Channel-specific settings
    pub settings: HashMap<String, serde_json::Value>,
}

impl ChannelConfig {
    /// Create a new channel config
    pub fn new(channel_type: impl Into<String>, identifier: impl Into<String>) -> Self {
        Self {
            channel_type: channel_type.into(),
            identifier: identifier.into(),
            enabled: true,
            settings: HashMap::new(),
        }
    }
}

/// Memory configuration for an assistant
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssistantMemory {
    /// Whether to use procedural memory
    pub procedural_enabled: bool,
    /// Whether to use user model
    pub user_model_enabled: bool,
    /// Custom memory files
    pub custom_files: Vec<PathBuf>,
    /// Vector store configuration
    pub vector_store: Option<VectorStoreConfig>,
}

/// Vector store configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorStoreConfig {
    /// Embedding model to use
    pub embedding_model: String,
    /// Vector dimension
    pub dimension: usize,
    /// Maximum vectors
    pub max_vectors: usize,
}

/// Configuration for spawning an assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantConfig {
    /// Assistant name (display name)
    pub name: String,
    /// Assistant specialization type
    pub assistant_type: AssistantType,
    /// System prompt (overrides default if provided)
    pub system_prompt: Option<String>,
    /// Tools available to this assistant
    pub tools: Option<Vec<String>>,
    /// Channels this assistant should listen on
    pub channels: Vec<ChannelConfig>,
    /// Memory configuration
    pub memory: AssistantMemory,
    /// Resource limits
    pub resource_limits: ResourceLimits,
    /// Parent assistant ID (None for root)
    pub parent_id: Option<String>,
    /// Environment variables
    pub environment: HashMap<String, String>,
}

impl AssistantConfig {
    /// Create a new assistant configuration
    pub fn new(name: impl Into<String>, assistant_type: AssistantType) -> Self {
        Self {
            name: name.into(),
            assistant_type,
            system_prompt: None,
            tools: None,
            channels: Vec::new(),
            memory: AssistantMemory::default(),
            resource_limits: ResourceLimits::default(),
            parent_id: None,
            environment: HashMap::new(),
        }
    }

    /// Set custom system prompt
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set tools
    pub fn with_tools(mut self, tools: Vec<String>) -> Self {
        self.tools = Some(tools);
        self
    }

    /// Add a channel
    pub fn with_channel(mut self, channel: ChannelConfig) -> Self {
        self.channels.push(channel);
        self
    }

    /// Set parent ID
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Set resource limits
    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.resource_limits = limits;
        self
    }

    /// Get the effective system prompt
    pub fn effective_system_prompt(&self) -> String {
        self.system_prompt
            .clone()
            .unwrap_or_else(|| self.assistant_type.default_system_prompt())
    }

    /// Get the effective tools
    pub fn effective_tools(&self) -> Vec<String> {
        self.tools
            .clone()
            .unwrap_or_else(|| self.assistant_type.default_tools())
    }
}

/// Resource limits for an assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum iterations per request
    pub max_iterations: usize,
    /// Maximum memory usage in MB
    pub max_memory_mb: usize,
    /// Maximum requests per minute
    pub max_requests_per_minute: u32,
    /// Maximum tokens per day
    pub max_tokens_per_day: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            max_memory_mb: 512,
            max_requests_per_minute: 30,
            max_tokens_per_day: 100_000,
        }
    }
}

/// A spawned persistent assistant
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentAssistant {
    /// Unique ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Specialization type
    pub assistant_type: AssistantType,
    /// System prompt
    pub system_prompt: String,
    /// Available tools
    pub tools: Vec<String>,
    /// Memory configuration
    pub memory: AssistantMemory,
    /// Channels
    pub channels: Vec<ChannelConfig>,
    /// Parent assistant ID (if spawned by another)
    pub parent_id: Option<String>,
    /// Creation time
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Current status
    pub status: AssistantStatus,
    /// Resource limits
    pub resource_limits: ResourceLimits,
    /// Data directory
    pub data_dir: PathBuf,
}

/// Assistant status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssistantStatus {
    /// Starting up
    Starting,
    /// Running normally
    Running,
    /// Paused/suspended
    Paused,
    /// Error state
    Error,
    /// Shutting down
    Stopping,
    /// Terminated
    Terminated,
}

impl PersistentAssistant {
    /// Create from configuration
    pub fn from_config(config: AssistantConfig, data_dir: PathBuf) -> Self {
        let id = Uuid::new_v4().to_string();
        // Compute these first before moving fields
        let system_prompt = config.effective_system_prompt();
        let tools = config.effective_tools();
        Self {
            id,
            name: config.name,
            assistant_type: config.assistant_type,
            system_prompt,
            tools,
            memory: config.memory,
            channels: config.channels,
            parent_id: config.parent_id,
            created_at: chrono::Utc::now(),
            status: AssistantStatus::Starting,
            resource_limits: config.resource_limits,
            data_dir,
        }
    }

    /// Get configuration representation
    pub fn to_config(&self) -> AssistantConfig {
        AssistantConfig {
            name: self.name.clone(),
            assistant_type: self.assistant_type.clone(),
            system_prompt: Some(self.system_prompt.clone()),
            tools: Some(self.tools.clone()),
            channels: self.channels.clone(),
            memory: self.memory.clone(),
            resource_limits: self.resource_limits.clone(),
            parent_id: self.parent_id.clone(),
            environment: HashMap::new(),
        }
    }
}

/// Assistant spawner for creating and managing assistants
#[derive(Debug)]
pub struct AssistantSpawner {
    /// Base directory for assistant data
    base_dir: PathBuf,
    /// Spawned assistants
    assistants: Arc<RwLock<HashMap<String, PersistentAssistant>>>,
    /// Process manager for actual subprocess management
    process_manager: process::ProcessManager,
}

impl AssistantSpawner {
    /// Create a new assistant spawner
    pub async fn new(base_dir: PathBuf) -> crate::Result<Self> {
        tokio::fs::create_dir_all(&base_dir).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to create assistants directory: {:?}", base_dir),
                details: e.to_string(),
            }
        })?;

        Ok(Self {
            base_dir,
            assistants: Arc::new(RwLock::new(HashMap::new())),
            process_manager: process::ProcessManager::new(),
        })
    }

    /// Create with default location
    pub async fn default() -> crate::Result<Self> {
        // Use centralized ~/.manta/agents directory
        let base_dir = crate::dirs::agents_dir();
        Self::new(base_dir).await
    }

    /// Spawn a new persistent assistant
    pub async fn spawn(&self, config: AssistantConfig) -> crate::Result<PersistentAssistant> {
        let assistant_id = Uuid::new_v4().to_string();
        let data_dir = self.base_dir.join(&assistant_id);

        // Create assistant data directory
        tokio::fs::create_dir_all(&data_dir).await.map_err(|e| {
            crate::error::MantaError::Storage {
                context: format!("Failed to create assistant directory: {:?}", data_dir),
                details: e.to_string(),
            }
        })?;

        // Create subdirectories
        for subdir in &["memory", "skills", "logs"] {
            tokio::fs::create_dir_all(data_dir.join(subdir))
                .await
                .map_err(|e| crate::error::MantaError::Storage {
                    context: format!("Failed to create {} directory", subdir),
                    details: e.to_string(),
                })?;
        }

        // Save configuration first (before consuming config)
        let config_path = data_dir.join("config.yaml");
        let config_yaml = serde_yaml::to_string(&config)?;
        tokio::fs::write(&config_path, config_yaml)
            .await
            .map_err(crate::error::MantaError::Io)?;

        // Create the assistant (config is moved here)
        let assistant = PersistentAssistant::from_config(config, data_dir);

        // Store in registry
        {
            let mut assistants = self.assistants.write().await;
            assistants.insert(assistant.id.clone(), assistant.clone());
        }

        info!(
            "Spawned assistant: {} (id: {}, type: {})",
            assistant.name, assistant.id, assistant.assistant_type
        );

        // Start the assistant (in background)
        let assistant_config = assistant.to_config();
        self.start_assistant(&assistant.id, &assistant_config)
            .await?;

        Ok(assistant)
    }

    /// Start an assistant process
    async fn start_assistant(
        &self,
        assistant_id: &str,
        config: &AssistantConfig,
    ) -> crate::Result<()> {
        let assistant = {
            let assistants = self.assistants.read().await;
            assistants.get(assistant_id).cloned().ok_or_else(|| {
                crate::error::MantaError::NotFound {
                    resource: format!("Assistant {}", assistant_id),
                }
            })?
        };

        debug!("Starting assistant process: {}", assistant_id);

        // Spawn the actual process
        self.process_manager.start(&assistant, config).await?;

        // Mark as running
        self.update_status(assistant_id, AssistantStatus::Running)
            .await;

        info!("Assistant {} started successfully (process spawned)", assistant_id);
        Ok(())
    }

    /// List all managed assistants
    pub async fn list_assistants(&self) -> Vec<PersistentAssistant> {
        let assistants = self.assistants.read().await;
        assistants.values().cloned().collect()
    }

    /// Get a specific assistant
    pub async fn get_assistant(&self, id: &str) -> Option<PersistentAssistant> {
        let assistants = self.assistants.read().await;
        assistants.get(id).cloned()
    }

    /// Send a message to a specific assistant
    pub async fn message_assistant(
        &self,
        assistant_id: &str,
        message: &str,
    ) -> crate::Result<String> {
        let assistant = self.get_assistant(assistant_id).await.ok_or_else(|| {
            crate::error::MantaError::NotFound {
                resource: format!("Assistant {}", assistant_id),
            }
        })?;

        if assistant.status != AssistantStatus::Running {
            return Err(crate::error::MantaError::Validation(format!(
                "Assistant {} is not running (status: {:?})",
                assistant_id, assistant.status
            )));
        }

        // Verify the process is actually running
        if !self.process_manager.is_running(assistant_id).await {
            self.update_status(assistant_id, AssistantStatus::Error)
                .await;
            return Err(crate::error::MantaError::ExternalService {
                source: format!("assistant-{}: Assistant process is not running", assistant_id),
                cause: None,
            });
        }

        debug!(
            "Sending message to assistant {}: {}",
            assistant_id,
            message.chars().take(50).collect::<String>()
        );

        // Send message via process manager (real IPC)
        let context = HashMap::new();
        let response = self
            .process_manager
            .send_message(assistant_id, message, context)
            .await?;

        info!("Received response from assistant {} ({} chars)", assistant_id, response.len());
        Ok(response)
    }

    /// Terminate an assistant
    pub async fn terminate(&self, assistant_id: &str) -> crate::Result<()> {
        let _assistant = self.get_assistant(assistant_id).await.ok_or_else(|| {
            crate::error::MantaError::NotFound {
                resource: format!("Assistant {}", assistant_id),
            }
        })?;

        info!("Terminating assistant: {}", assistant_id);

        self.update_status(assistant_id, AssistantStatus::Stopping)
            .await;

        // Stop the actual process
        self.process_manager
            .stop(assistant_id, Some("Terminated by parent".to_string()))
            .await?;

        // Remove from registry
        {
            let mut assistants = self.assistants.write().await;
            assistants.remove(assistant_id);
        }

        info!("Assistant {} terminated", assistant_id);
        Ok(())
    }

    /// Pause/suspend an assistant
    pub async fn pause(&self, assistant_id: &str) -> crate::Result<()> {
        self.update_status(assistant_id, AssistantStatus::Paused)
            .await;
        info!("Assistant {} paused", assistant_id);
        Ok(())
    }

    /// Resume a paused assistant
    pub async fn resume(&self, assistant_id: &str) -> crate::Result<()> {
        self.update_status(assistant_id, AssistantStatus::Running)
            .await;
        info!("Assistant {} resumed", assistant_id);
        Ok(())
    }

    /// Update assistant status
    async fn update_status(&self, assistant_id: &str, status: AssistantStatus) {
        let mut assistants = self.assistants.write().await;
        if let Some(assistant) = assistants.get_mut(assistant_id) {
            assistant.status = status;
        }
    }

    /// Get assistants by parent ID
    pub async fn get_children(&self, parent_id: &str) -> Vec<PersistentAssistant> {
        let assistants = self.assistants.read().await;
        assistants
            .values()
            .filter(|a| a.parent_id.as_ref() == Some(&parent_id.to_string()))
            .cloned()
            .collect()
    }

    /// Check if assistant exists
    pub async fn exists(&self, assistant_id: &str) -> bool {
        let assistants = self.assistants.read().await;
        assistants.contains_key(assistant_id)
    }
}

/// Assistant mesh for inter-assistant communication
pub mod mesh;
/// Process management for spawned assistants
pub mod process;

/// Tool for spawning assistants
pub mod tool {
    use super::*;
    use crate::tools::{Tool, ToolContext, ToolExecutionResult};

    /// Tool for managing persistent assistants
    #[derive(Debug)]
    pub struct AssistantTool {
        spawner: AssistantSpawner,
    }

    impl AssistantTool {
        /// Create a new assistant tool
        pub fn new(spawner: AssistantSpawner) -> Self {
            Self { spawner }
        }
    }

    #[async_trait]
    impl Tool for AssistantTool {
        fn name(&self) -> &str {
            "assistant"
        }

        fn description(&self) -> &str {
            r#"Spawn and manage persistent specialized assistants.

Use this to create other AI assistants with specific roles:
- Researcher: Deep research and analysis
- CodeReviewer: Code review and PR analysis
- Scheduler: Calendar and time management
- Social: Social media interactions
- Specialist: Custom specialization

Spawned assistants have:
- Their own memory and configuration
- Isolated data directories
- Resource limits
- Optional channel connections

Examples:
- Spawn a researcher to gather information
- Create a code reviewer for a project
- Set up a scheduler for reminders"#
        }

        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["spawn", "list", "message", "terminate", "pause", "resume"],
                        "description": "Action to perform"
                    },
                    "name": {
                        "type": "string",
                        "description": "Assistant name (for spawn)"
                    },
                    "assistant_type": {
                        "type": "string",
                        "enum": ["researcher", "code_reviewer", "scheduler", "social"],
                        "description": "Type of assistant (for spawn)"
                    },
                    "specialization": {
                        "type": "string",
                        "description": "Custom specialization (for specialist type)"
                    },
                    "system_prompt": {
                        "type": "string",
                        "description": "Custom system prompt (optional)"
                    },
                    "assistant_id": {
                        "type": "string",
                        "description": "Assistant ID (for message/terminate/pause/resume)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Message to send (for message action)"
                    }
                },
                "required": ["action"]
            })
        }

        async fn execute(
            &self,
            args: serde_json::Value,
            _context: &ToolContext,
        ) -> crate::Result<ToolExecutionResult> {
            let action = args["action"].as_str().ok_or_else(|| {
                crate::error::MantaError::Validation("action is required".to_string())
            })?;

            match action {
                "spawn" => {
                    let name = args["name"].as_str().ok_or_else(|| {
                        crate::error::MantaError::Validation(
                            "name is required for spawn".to_string(),
                        )
                    })?;

                    let assistant_type =
                        if let Some(specialization) = args["specialization"].as_str() {
                            AssistantType::Specialist(specialization.to_string())
                        } else {
                            let type_str = args["assistant_type"].as_str().unwrap_or("specialist");
                            match type_str {
                                "researcher" => AssistantType::Researcher,
                                "code_reviewer" => AssistantType::CodeReviewer,
                                "scheduler" => AssistantType::Scheduler,
                                "social" => AssistantType::Social,
                                _ => AssistantType::Specialist(type_str.to_string()),
                            }
                        };

                    let mut config = AssistantConfig::new(name, assistant_type);

                    if let Some(prompt) = args["system_prompt"].as_str() {
                        config = config.with_system_prompt(prompt);
                    }

                    let assistant = self.spawner.spawn(config).await?;

                    Ok(ToolExecutionResult::success(format!(
                        "Spawned assistant: {} (id: {})",
                        assistant.name, assistant.id
                    ))
                    .with_data(serde_json::json!({
                        "id": assistant.id,
                        "name": assistant.name,
                        "type": assistant.assistant_type.to_string(),
                        "status": assistant.status,
                        "data_dir": assistant.data_dir,
                    })))
                }

                "list" => {
                    let assistants = self.spawner.list_assistants().await;
                    let summary: Vec<serde_json::Value> = assistants
                        .iter()
                        .map(|a| {
                            serde_json::json!({
                                "id": a.id,
                                "name": a.name,
                                "type": a.assistant_type.to_string(),
                                "status": a.status,
                                "parent_id": a.parent_id,
                                "created_at": a.created_at.to_rfc3339(),
                            })
                        })
                        .collect();

                    Ok(ToolExecutionResult::success(format!(
                        "{} assistants found",
                        assistants.len()
                    ))
                    .with_data(serde_json::json!({
                        "assistants": summary,
                        "count": assistants.len(),
                    })))
                }

                "message" => {
                    let assistant_id = args["assistant_id"].as_str().ok_or_else(|| {
                        crate::error::MantaError::Validation(
                            "assistant_id is required for message".to_string(),
                        )
                    })?;
                    let message = args["message"].as_str().ok_or_else(|| {
                        crate::error::MantaError::Validation(
                            "message is required for message".to_string(),
                        )
                    })?;

                    let response = self
                        .spawner
                        .message_assistant(assistant_id, message)
                        .await?;

                    Ok(ToolExecutionResult::success(response))
                }

                "terminate" => {
                    let assistant_id = args["assistant_id"].as_str().ok_or_else(|| {
                        crate::error::MantaError::Validation(
                            "assistant_id is required for terminate".to_string(),
                        )
                    })?;

                    self.spawner.terminate(assistant_id).await?;

                    Ok(ToolExecutionResult::success(format!(
                        "Terminated assistant: {}",
                        assistant_id
                    )))
                }

                "pause" => {
                    let assistant_id = args["assistant_id"].as_str().ok_or_else(|| {
                        crate::error::MantaError::Validation(
                            "assistant_id is required for pause".to_string(),
                        )
                    })?;

                    self.spawner.pause(assistant_id).await?;

                    Ok(ToolExecutionResult::success(format!("Paused assistant: {}", assistant_id)))
                }

                "resume" => {
                    let assistant_id = args["assistant_id"].as_str().ok_or_else(|| {
                        crate::error::MantaError::Validation(
                            "assistant_id is required for resume".to_string(),
                        )
                    })?;

                    self.spawner.resume(assistant_id).await?;

                    Ok(ToolExecutionResult::success(format!("Resumed assistant: {}", assistant_id)))
                }

                _ => {
                    Err(crate::error::MantaError::Validation(format!("Unknown action: {}", action)))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assistant_type_defaults() {
        let researcher = AssistantType::Researcher;
        assert_eq!(researcher.to_string(), "researcher");
        assert!(!researcher.default_system_prompt().is_empty());
        assert!(!researcher.default_tools().is_empty());

        let specialist = AssistantType::Specialist("custom".to_string());
        assert_eq!(specialist.to_string(), "specialist:custom");
    }

    #[test]
    fn test_assistant_config() {
        let config = AssistantConfig::new("TestBot", AssistantType::Researcher)
            .with_system_prompt("Custom prompt")
            .with_tools(vec!["tool1".to_string()]);

        assert_eq!(config.name, "TestBot");
        assert_eq!(config.effective_system_prompt(), "Custom prompt");
        assert_eq!(config.effective_tools(), vec!["tool1".to_string()]);
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.max_iterations, 50);
        assert_eq!(limits.max_memory_mb, 512);
    }

    #[test]
    fn test_channel_config() {
        let config = ChannelConfig::new("telegram", "@test_bot");
        assert_eq!(config.channel_type, "telegram");
        assert_eq!(config.identifier, "@test_bot");
        assert!(config.enabled);
    }
}
