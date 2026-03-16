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
            .arg("start")
            .arg("--host").arg(&self.config.host)
            .arg("--port").arg(self.config.port.to_string())
            .arg("--web-port").arg(self.config.web_port.to_string())
            .arg("--foreground")
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

    /// Run the daemon in the foreground with Gateway (new architecture)
    pub async fn run_foreground(&self) -> crate::Result<()> {
        println!("🚀 Manta daemon running with Gateway...");

        use crate::gateway::{Gateway, GatewayConfig};
        use crate::agent::AgentConfig;

        // Build GatewayConfig from daemon config and environment
        let mut gateway_config = GatewayConfig {
            host: self.config.host.clone(),
            port: self.config.port,
            web_port: self.config.web_port,
            tailscale_enabled: false,
            tailscale_domain: None,
            default_agent: AgentConfig::default(),
            channels: std::collections::HashMap::new(),
            vector_memory: Default::default(),
            plugins: Default::default(),
            hot_reload: Default::default(),
            acp: Default::default(),
            providers: std::collections::HashMap::new(),
            model: std::env::var("MANTA_MODEL").unwrap_or_else(|_| "claude-3-sonnet-20240229".to_string()),
            model_provider: std::env::var("MANTA_MODEL_PROVIDER").unwrap_or_else(|_| "anthropic".to_string()),
        };

        // Enable features based on environment variables
        // Vector Memory
        if std::env::var("MANTA_VECTOR_MEMORY_ENABLED").map(|v| v == "true" || v == "1").unwrap_or(false) {
            gateway_config.vector_memory.enabled = true;
            gateway_config.vector_memory.embedding_api_key = std::env::var("MANTA_EMBEDDING_API_KEY").ok();
            if let Ok(model) = std::env::var("MANTA_EMBEDDING_MODEL") {
                gateway_config.vector_memory.embedding_model = model;
            }
        }

        // Plugins
        if std::env::var("MANTA_PLUGINS_ENABLED").map(|v| v == "false" || v == "0").unwrap_or(false) {
            gateway_config.plugins.enabled = false;
        }

        // Hot Reload
        if std::env::var("MANTA_HOT_RELOAD_ENABLED").map(|v| v == "false" || v == "0").unwrap_or(false) {
            gateway_config.hot_reload.enabled = false;
        }

        // ACP
        if std::env::var("MANTA_ACP_ENABLED").map(|v| v == "false" || v == "0").unwrap_or(false) {
            gateway_config.acp.enabled = false;
        }

        // Configure LLM Provider from environment variables (legacy support)
        if let (Ok(base_url), Ok(api_key)) = (std::env::var("MANTA_BASE_URL"), std::env::var("MANTA_API_KEY")) {
            let is_anthropic = std::env::var("MANTA_IS_ANTHROPIC")
                .map(|v| v.to_lowercase() == "true" || v == "1")
                .unwrap_or(false);

            let provider_type = if is_anthropic {
                crate::model_router::ProviderType::Anthropic
            } else {
                crate::model_router::ProviderType::OpenAi
            };

            let provider_config = crate::model_router::ProviderConfig {
                provider_type,
                api_key,
                base_url: Some(base_url),
                timeout: std::time::Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };

            let provider_name = if is_anthropic { "anthropic" } else { "openai" };
            gateway_config.providers.insert(provider_name.to_string(), provider_config);
            println!("🤖 Configured {} provider from environment", provider_name);
        } else if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
            // Also support direct ANTHROPIC_API_KEY
            let provider_config = crate::model_router::ProviderConfig {
                provider_type: crate::model_router::ProviderType::Anthropic,
                api_key,
                base_url: None,
                timeout: std::time::Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };
            gateway_config.providers.insert("anthropic".to_string(), provider_config);
            println!("🤖 Configured Anthropic provider from ANTHROPIC_API_KEY");
        } else if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            // Support OPENAI_API_KEY
            let provider_config = crate::model_router::ProviderConfig {
                provider_type: crate::model_router::ProviderType::OpenAi,
                api_key,
                base_url: None,
                timeout: std::time::Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };
            gateway_config.providers.insert("openai".to_string(), provider_config);
            println!("🤖 Configured OpenAI provider from OPENAI_API_KEY");
        }

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

        // Create and start the Gateway
        let gateway = Gateway::new(gateway_config.clone()).await?;

        println!("✅ Gateway ready");
        println!("   API: http://{}:{}", gateway_config.host, gateway_config.port);
        println!("   Web: http://{}:{}", gateway_config.host, gateway_config.web_port);

        gateway.start().await
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
