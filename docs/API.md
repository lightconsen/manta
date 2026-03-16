# Manta API Documentation

This document describes the internal APIs and extension points for Manta.

## Table of Contents

- [Core Traits](#core-traits)
- [Tool System](#tool-system)
- [Provider API](#provider-api)
- [Memory System](#memory-system)
- [Skills System](#skills-system)
- [Assistant Mesh](#assistant-mesh)

## Core Traits

### Tool Trait

The `Tool` trait is the foundation of Manta's extensibility:

```rust
use async_trait::async_trait;
use serde_json::Value;
use manta::tools::{Tool, ToolContext, ToolExecutionResult};

#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name
    fn name(&self) -> &str;

    /// Description for LLM
    fn description(&self) -> &str;

    /// JSON Schema for parameters
    fn parameters_schema(&self) -> Value;

    /// Execute the tool
    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> manta::Result<ToolExecutionResult>;

    /// Check availability
    fn is_available(&self, _context: &ToolContext) -> bool {
        true
    }
}
```

### Registering a Custom Tool

```rust
use manta::tools::ToolRegistry;

// Create registry
let mut registry = ToolRegistry::new();

// Register custom tool
registry.register(Box::new(MyCustomTool::new()));

// Use in agent
let agent = Agent::new(provider, registry);
```

## Tool System

### Built-in Tools

| Tool | Name | Description |
|------|------|-------------|
| FileReadTool | `read_file` | Read file contents |
| FileWriteTool | `write_file` | Write file contents |
| FileEditTool | `edit_file` | Edit files with replacements |
| GlobTool | `glob` | Search files by pattern |
| ShellTool | `shell` | Execute shell commands |
| WebSearchTool | `web_search` | Search the web |
| WebFetchTool | `web_fetch` | Fetch web page content |
| TodoTool | `todo` | Manage tasks |
| CodeExecutionTool | `execute_code` | Run Python code |
| DelegateTool | `delegate` | Spawn subagents |
| McpConnectionTool | `mcp` | Connect to MCP servers |

### ToolContext

Tools receive context during execution:

```rust
pub struct ToolContext {
    pub user_id: String,
    pub conversation_id: String,
    pub working_directory: PathBuf,
    pub environment: HashMap<String, String>,
    pub timeout: Duration,
    pub allowed_paths: Vec<PathBuf>,
    pub allowed_commands: Vec<String>,
    pub sandboxed: bool,
}
```

### Creating a Custom Tool

```rust
use manta::tools::{Tool, ToolContext, ToolExecutionResult, create_schema};
use async_trait::async_trait;
use serde_json::json;

pub struct WeatherTool {
    api_key: String,
}

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str {
        "get_weather"
    }

    fn description(&self) -> &str {
        "Get weather for a location"
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Get current weather",
            json!({
                "location": {
                    "type": "string",
                    "description": "City name or coordinates"
                }
            }),
            vec!["location"],
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> manta::Result<ToolExecutionResult> {
        let location = args["location"].as_str()
            .ok_or_else(|| MantaError::Validation("location required".into()))?;

        // Fetch weather...
        let weather = self.fetch_weather(location).await?;

        Ok(ToolExecutionResult::success(weather))
    }
}
```

## Provider API

### Provider Trait

```rust
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider name
    fn name(&self) -> &str;

    /// Check if models are supported
    fn supports_models(&self) -> bool;

    /// List available models
    async fn list_models(&self) -> Result<Vec<Model>>;

    /// Send messages and get response
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse>;

    /// Stream response
    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk>>>;
}
```

### Creating a Custom Provider

```rust
use manta::providers::{Provider, CompletionRequest, CompletionResponse};

pub struct CustomProvider {
    client: reqwest::Client,
    api_key: String,
}

#[async_trait]
impl Provider for CustomProvider {
    fn name(&self) -> &str {
        "custom"
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse> {
        // Implement API call
        todo!()
    }
}
```

## Memory System

### MemoryStore Trait

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Store a value
    async fn set(&self, key: &str, value: &str) -> Result<()>;

    /// Retrieve a value
    async fn get(&self, key: &str) -> Result<Option<String>>;

    /// Delete a value
    async fn delete(&self, key: &str) -> Result<()>;

    /// Search stored values
    async fn search(&self, query: &str) -> Result<Vec<MemoryEntry>>;
}
```

### Dual Memory

```rust
use manta::memory::DualMemory;

// Initialize with paths
let memory = DualMemory::load(
    "~/.manta/memory/agent.md",
    "~/.manta/memory/user.md",
).await?;

// Get memories for prompt injection
let memories = memory.get_memories().await;
```

## Skills System

### Skill Structure

```rust
use manta::skills::{Skill, SkillTrigger};

let skill = Skill {
    name: "weather".to_string(),
    description: "Get weather information".to_string(),
    triggers: vec![
        SkillTrigger::Keyword("weather".to_string()),
        SkillTrigger::Regex(regex::Regex::new(r"weather in (.+)").unwrap()),
    ],
    prompt: "When user asks about weather...".to_string(),
    config: None,
    examples: vec![],
};
```

### Loading Skills

```rust
use manta::skills::SkillManager;

let manager = SkillManager::new();

// Load from directory
manager.load_from_dir("~/.manta/skills").await?;

// Get matching skill
if let Some(skill) = manager.find_matching_skill("what's the weather?") {
    println!("Found skill: {}", skill.name);
}
```

## Assistant Mesh

### Mesh Communication

```rust
use manta::assistants::mesh::{AssistantMesh, MeshMessage};

// Create mesh
let mesh = AssistantMesh::new();

// Register assistant
let rx = mesh.register("assistant_1").await;

// Send message
mesh.send("assistant_1", "assistant_2", "Hello!").await?;

// Broadcast
mesh.broadcast("assistant_1", "System update").await?;
```

### MeshMessage

```rust
pub struct MeshMessage {
    pub id: String,
    pub from: String,
    pub to: Option<String>,  // None for broadcast
    pub content: String,
    pub msg_type: MessageType,  // Direct, Broadcast, Request, Response, Event
    pub timestamp: DateTime<Utc>,
    pub reply_to: Option<String>,
}
```

## Event System

### Agent Events

```rust
use manta::agent::AgentEvent;

match event {
    AgentEvent::ToolCall { name, args } => {
        println!("Tool called: {} with {:?}", name, args);
    }
    AgentEvent::ToolResult { name, result } => {
        println!("Tool result: {} = {:?}", name, result);
    }
    AgentEvent::BudgetWarning { remaining } => {
        println!("Budget warning: {} remaining", remaining);
    }
    AgentEvent::Complete { response } => {
        println!("Complete: {}", response);
    }
}
```

## Configuration API

### Programmatic Configuration

```rust
use manta::config::Config;

let config = Config::builder()
    .provider(ProviderConfig::OpenAi {
        api_key: std::env::var("OPENAI_API_KEY")?,
        model: "gpt-4o".to_string(),
    })
    .agent(AgentConfig {
        name: "CustomAgent".to_string(),
        system_prompt: "You are a specialist...".to_string(),
    })
    .security(SecurityConfig {
        allow_shell: true,
        sandboxed: true,
        max_budget: 100,
    })
    .build();
```

## Error Handling

### MantaError

```rust
use manta::error::MantaError;

match result {
    Err(MantaError::NotFound { resource }) => {
        eprintln!("Resource not found: {}", resource);
    }
    Err(MantaError::Validation(msg)) => {
        eprintln!("Validation error: {}", msg);
    }
    Err(MantaError::ExternalService { source, cause }) => {
        eprintln!("External error: {} - {:?}", source, cause);
    }
    _ => {}
}
```

## Best Practices

1. **Tool Development**
   - Keep tools focused on single responsibility
   - Validate all inputs
   - Return clear error messages
   - Set reasonable timeouts

2. **Memory Usage**
   - Don't store large values in memory
   - Use compression for large content
   - Regularly clean up old sessions

3. **Security**
   - Always validate paths before file operations
   - Sanitize shell commands
   - Use sandboxing for untrusted code
   - Rate limit expensive operations

4. **Performance**
   - Cache expensive computations
   - Use streaming for large responses
   - Pool database connections
   - Profile before optimizing
