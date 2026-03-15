//! Daemon management for Manta
//!
//! Provides start/stop/status functionality for running Manta as a background service.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;

/// Daemon configuration
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Host to bind to
    pub host: String,
    /// API port to listen on
    pub port: u16,
    /// Web terminal port
    pub web_port: u16,
    /// Path to PID file
    pub pid_file: PathBuf,
}

/// Daemon manager
pub struct DaemonManager {
    config: DaemonConfig,
}

impl DaemonManager {
    /// Create a new daemon manager
    pub fn new(config: DaemonConfig) -> crate::Result<Self> {
        Ok(Self { config })
    }

    /// Start the daemon in the background
    pub async fn start(&self) -> crate::Result<()> {
        // Check if already running
        if let Some(pid) = self.read_pid().await {
            if self.is_process_running(pid).await {
                println!("✅ Manta daemon is already running (PID: {})", pid);
                return Ok(());
            }
            // Stale PID file, remove it
            let _ = tokio::fs::remove_file(&self.config.pid_file).await;
        }

        // Get the current executable path
        let exe_path = std::env::current_exe()
            .map_err(|e| crate::error::MantaError::Io(e))?;

        // Get log file path
        let log_path = crate::logs::log_file_path();

        // Ensure log directory exists
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Open log file for appending (std version for process spawning)
        let log_file_std = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| crate::error::MantaError::Io(e))?;

        // Spawn the daemon process with output redirected to log file
        let child = Command::new(&exe_path)
            .arg("daemon")
            .arg("--host").arg(&self.config.host)
            .arg("--port").arg(self.config.port.to_string())
            .arg("--web-port").arg(self.config.web_port.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(log_file_std.try_clone().map_err(|e| crate::error::MantaError::Io(e))?)
            .stderr(log_file_std)
            .spawn()
            .map_err(|e| crate::error::MantaError::Io(e))?;

        let pid = child.id().expect("Failed to get child PID");

        // Write PID file
        self.write_pid(pid).await?;

        println!("✅ Manta daemon started (PID: {})", pid);
        println!("   Host: {}", self.config.host);
        println!("   API Port: {}", self.config.port);
        println!("   Web Port: {}", self.config.web_port);
        println!("   API: http://{}:{}", self.config.host, self.config.port);
        println!("   Web: http://{}:{}", self.config.host, self.config.web_port);
        println!("   Logs: {:?}", log_path);

        Ok(())
    }

    /// Run the daemon in the foreground with AI agent
    pub async fn run_foreground(&self) -> crate::Result<()> {
        println!("🚀 Manta daemon running...");

        // Import server modules
        use crate::server::{ServerConfig, start_server_with_agent};
        use crate::core::Engine;
        use crate::agent::{AgentConfig, AgentBuilder};
        use crate::tools::{ToolRegistry, ShellTool, FileReadTool, FileWriteTool, FileEditTool, GlobTool, TodoTool, MemoryTool, CronTool};

        // Load environment variables for AI provider
        let base_url = std::env::var("MANTA_BASE_URL");
        let api_key = std::env::var("MANTA_API_KEY");
        let model = std::env::var("MANTA_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

        // Create engine
        let engine = Arc::new(Engine::new());

        // Create agent if environment variables are set
        let agent = if let (Ok(base_url), Ok(api_key)) = (base_url, api_key) {
            println!("🤖 Initializing AI agent...");

            let is_anthropic = std::env::var("MANTA_IS_ANTHROPIC")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false);

            // Create provider
            let provider: Arc<dyn crate::providers::Provider> = if is_anthropic {
                use crate::providers::anthropic::AnthropicProvider;
                Arc::new(AnthropicProvider::with_base_url(api_key, base_url)?.with_model(model))
            } else {
                use crate::providers::openai::OpenAiProvider;
                Arc::new(OpenAiProvider::with_base_url(api_key, base_url)?.with_model(model))
            };

            // Initialize SQLite memory store for persistent memory
            let memory_store = Arc::new(Self::init_memory_store().await?);

            // Create tool registry
            let mut tool_registry = ToolRegistry::new();
            tool_registry.register(Box::new(ShellTool::new()));
            tool_registry.register(Box::new(FileReadTool::new()));
            tool_registry.register(Box::new(FileWriteTool::new()));
            tool_registry.register(Box::new(FileEditTool::new()));
            tool_registry.register(Box::new(GlobTool::new()));
            tool_registry.register(Box::new(TodoTool::new()));

            // Initialize memory tool with shared SQLite storage
            let memory_tool = MemoryTool::with_store(memory_store.clone()).await?;
            tool_registry.register(Box::new(memory_tool));

            // Initialize session search for conversation history search
            let session_search = Arc::new(crate::memory::SessionSearch::new(memory_store.pool()));
            session_search.initialize().await?;
            let session_search_tool = crate::memory::session_search::tool::SessionSearchTool::new((*session_search).clone());
            tool_registry.register(Box::new(session_search_tool));

            tool_registry.register(Box::new(CronTool::new()));

            // Add MCP tool
            let mcp_tool = crate::tools::McpConnectionTool::new();
            tool_registry.register(Box::new(mcp_tool));

            // Load skills
            let skills_prompt = crate::cli::Cli::load_skills_prompt().await.ok().flatten();

            // Build agent
            let agent_config = AgentConfig::default();
            let mut builder = AgentBuilder::new()
                .config(agent_config);

            if let Some(prompt) = skills_prompt {
                builder = builder.skills(prompt);
            }

            let agent = builder
                .provider(provider)
                .tools(Arc::new(tool_registry))
                .memory_store(memory_store.clone())
                .chat_history(memory_store)
                .session_search(session_search)
                .build()?;

            println!("✅ AI agent ready");
            Some(Arc::new(agent))
        } else {
            println!("⚠️ AI agent not configured (set MANTA_BASE_URL and MANTA_API_KEY)");
            None
        };

        let server_config = ServerConfig {
            host: self.config.host.clone(),
            port: self.config.port,
            web_port: self.config.web_port,
        };

        // Write PID file
        let pid = std::process::id();
        self.write_pid(pid).await?;

        // Clean up PID file on shutdown
        let pid_file = self.config.pid_file.clone();
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            let _ = tokio::fs::remove_file(&pid_file).await;
            println!("\n👋 Daemon stopped");
        });

        // Start the server with agent
        match agent {
            Some(agent) => start_server_with_agent(server_config, engine, agent).await,
            None => start_server_with_agent(server_config, engine, Arc::new(AgentBuilder::new().build()?)).await,
        }
    }

    /// Stop the daemon gracefully
    pub async fn stop(&self) -> crate::Result<()> {
        match self.read_pid().await {
            Some(pid) => {
                if self.is_process_running(pid).await {
                    // Send SIGTERM
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;

                        kill(Pid::from_raw(pid as i32), Signal::SIGTERM)
                            .map_err(|e| crate::error::MantaError::Internal(
                                format!("Failed to send signal: {}", e)
                            ))?;
                    }

                    #[cfg(not(unix))]
                    {
                        // Windows: use taskkill
                        Command::new("taskkill")
                            .args(["/PID", &pid.to_string(), "/F"])
                            .output()
                            .await
                            .map_err(|e| crate::error::MantaError::Io(e))?;
                    }

                    // Wait for process to exit
                    for _ in 0..50 {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        if !self.is_process_running(pid).await {
                            break;
                        }
                    }

                    // Remove PID file
                    let _ = tokio::fs::remove_file(&self.config.pid_file).await;
                    println!("✅ Manta daemon stopped");
                } else {
                    println!("⚠️ Daemon was not running (removing stale PID file)");
                    let _ = tokio::fs::remove_file(&self.config.pid_file).await;
                }
                Ok(())
            }
            None => {
                println!("⚠️ Manta daemon is not running");
                Ok(())
            }
        }
    }

    /// Force stop the daemon (SIGKILL)
    pub async fn stop_force(&self) -> crate::Result<()> {
        match self.read_pid().await {
            Some(pid) => {
                if self.is_process_running(pid).await {
                    // Send SIGKILL
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;

                        kill(Pid::from_raw(pid as i32), Signal::SIGKILL)
                            .map_err(|e| crate::error::MantaError::Internal(
                                format!("Failed to send signal: {}", e)
                            ))?;
                    }

                    #[cfg(not(unix))]
                    {
                        Command::new("taskkill")
                            .args(["/PID", &pid.to_string(), "/F"])
                            .output()
                            .await
                            .map_err(|e| crate::error::MantaError::Io(e))?;
                    }

                    println!("✅ Manta daemon force stopped");
                } else {
                    println!("⚠️ Daemon was not running");
                }

                // Remove PID file
                let _ = tokio::fs::remove_file(&self.config.pid_file).await;
                Ok(())
            }
            None => {
                println!("⚠️ Manta daemon is not running");
                Ok(())
            }
        }
    }

    /// Check daemon status
    pub async fn status(&self) -> crate::Result<()> {
        match self.read_pid().await {
            Some(pid) => {
                if self.is_process_running(pid).await {
                    println!("✅ Manta daemon is running");
                    println!("   PID: {}", pid);
                    println!("   Host: {}", self.config.host);
                    println!("   API Port: {}", self.config.port);
                    println!("   Web Port: {}", self.config.web_port);
                    println!("   API: http://{}:{}", self.config.host, self.config.port);
                    println!("   Web: http://{}:{}", self.config.host, self.config.web_port);
                    println!("   PID file: {:?}", self.config.pid_file);
                } else {
                    println!("⚠️ Daemon is not running (stale PID file)");
                    let _ = tokio::fs::remove_file(&self.config.pid_file).await;
                }
                Ok(())
            }
            None => {
                println!("⚠️ Manta daemon is not running");
                Ok(())
            }
        }
    }

    /// Read PID from file
    async fn read_pid(&self) -> Option<u32> {
        match tokio::fs::read_to_string(&self.config.pid_file).await {
            Ok(content) => content.trim().parse::<u32>().ok(),
            Err(_) => None,
        }
    }

    /// Write PID to file
    async fn write_pid(&self, pid: u32) -> crate::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.config.pid_file.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| crate::error::MantaError::Io(e))?;
        }

        tokio::fs::write(&self.config.pid_file, pid.to_string())
            .await
            .map_err(|e| crate::error::MantaError::Io(e))?;

        Ok(())
    }

    /// Check if a process is running
    async fn is_process_running(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {

            use nix::unistd::Pid;

            // Send signal 0 to check if process exists
            nix::sys::signal::kill(Pid::from_raw(pid as i32), None).is_ok()
        }

        #[cfg(not(unix))]
        {
            // Windows: check via tasklist
            match Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", pid), "/NH"])
                .output()
                .await
            {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    stdout.contains(&pid.to_string())
                }
                Err(_) => false,
            }
        }
    }

    /// Initialize the SQLite memory store
    async fn init_memory_store() -> crate::Result<crate::memory::SqliteMemoryStore> {
        // Use centralized ~/.manta/memory directory
        let db_path = crate::dirs::default_memory_db();

        // Create the database file if it doesn't exist
        // SQLite requires the file to exist before connecting
        if !db_path.exists() {
            if let Some(parent) = db_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::File::create(&db_path).await?;
        }

        let db_url = format!("sqlite:{}", db_path.display());

        println!("💾 Memory store: {}", db_path.display());

        crate::memory::SqliteMemoryStore::new(&db_url).await
    }
}
