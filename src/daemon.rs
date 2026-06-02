//! Daemon management for Manta
//!
//! Provides start/stop/status functionality for running Manta as a background service.

use std::path::PathBuf;
use tokio::process::Command;
use tracing::warn;

/// Daemon configuration
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Host to bind to
    pub host: String,
    /// Port for gateway API, WebSocket, and SPA
    pub port: u16,
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
        let exe_path = std::env::current_exe().map_err(crate::error::MantaError::Io)?;

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
            .map_err(crate::error::MantaError::Io)?;

        // Spawn the daemon process with output redirected to log file
        let child = Command::new(&exe_path)
            .arg("start")
            .arg("--host")
            .arg(&self.config.host)
            .arg("--port")
            .arg(self.config.port.to_string())
            .arg("--foreground")
            .stdin(std::process::Stdio::null())
            .stdout(
                log_file_std
                    .try_clone()
                    .map_err(crate::error::MantaError::Io)?,
            )
            .stderr(log_file_std)
            .spawn()
            .map_err(crate::error::MantaError::Io)?;

        let pid = child.id().expect("Failed to get child PID");

        // Write PID file
        self.write_pid(pid).await?;

        println!("✅ Manta daemon started (PID: {})", pid);
        println!("   Host: {}", self.config.host);
        println!("   Port: {}", self.config.port);
        println!("   URL: http://{}:{}", self.config.host, self.config.port);
        println!("   Logs: {:?}", log_path);

        Ok(())
    }

    /// Run the daemon in the foreground with Gateway (new architecture)
    pub async fn run_foreground(&self) -> crate::Result<()> {
        println!("🚀 Manta daemon running with Gateway...");

        use crate::gateway::{Gateway, GatewayConfig};

        // ── Auto-initialize ~/.manta directory and manta.toml ──────────────
        let manta_dir = crate::dirs::manta_dir();
        let config_path = manta_dir.join("manta.toml");

        if !manta_dir.exists() {
            println!("📁 Creating Manta directory at {:?}...", manta_dir);
            tokio::fs::create_dir_all(&manta_dir)
                .await
                .map_err(crate::error::MantaError::Io)?;
        }

        if !config_path.exists() {
            println!("📄 Creating default manta.toml at {:?}...", config_path);
            let default_config = r#"# Manta Configuration
# Auto-generated on first start

[server]
host = "127.0.0.1"
port = 18080

[security]
enabled = true
auth_required = false
pairing_required = false
auth_mode = "none"
shared_token = ""
security_headers = true

[security.rate_limit]
enabled = true
capacity = 100
refill_rate = 10

[model]
model = "claude-3-sonnet-20240229"
model_provider = "anthropic"

[storage]
storage_type = "sqlite"
connection = ""

[acp]
enabled = true
max_subagents = 10
default_timeout_seconds = 300

[cron]
enabled = true
check_interval_seconds = 60

[plugins]
enabled = true
auto_load = true

[hot_reload]
enabled = true
watch_config = true
watch_agents = true
watch_plugins = true
debounce_seconds = 2

[cost_guard]
daily_limit_cents = 0
hourly_action_limit = 0

# Workspace settings (restrict file operations to this directory)
# When workspace_dir is not set, it defaults to ~/.manta/workspace
# workspace_dir = "~/projects"
workspace_only = true
"#;
            tokio::fs::write(&config_path, default_config)
                .await
                .map_err(crate::error::MantaError::Io)?;
            println!("✅ Default config created. Edit {:?} to customize.", config_path);
        }

        // Try to load existing Gateway config from manta.toml
        let mut gateway_config = if config_path.exists() {
            match tokio::fs::read_to_string(&config_path).await {
                Ok(content) => {
                    match toml::from_str::<GatewayConfig>(&content) {
                        Ok(mut config) => {
                            // Override with daemon config for host/port
                            config.host = self.config.host.clone();
                            config.port = self.config.port;
                            println!("📄 Loaded Gateway config from {:?}", config_path);
                            println!("   Channels configured: {}", config.channels.len());
                            for (name, ch) in &config.channels {
                                println!("   - {}: enabled={}", name, ch.enabled);
                            }
                            config
                        }
                        Err(e) => {
                            warn!("Failed to parse manta.toml: {}, using defaults", e);
                            GatewayConfig::default()
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read manta.toml: {}, using defaults", e);
                    GatewayConfig::default()
                }
            }
        } else {
            println!("📄 No manta.toml found, using default config");
            GatewayConfig::default()
        };

        // Apply environment overrides
        gateway_config.host = self.config.host.clone();
        gateway_config.port = self.config.port;
        gateway_config.model =
            std::env::var("MANTA_MODEL").unwrap_or_else(|_| gateway_config.model.clone());
        gateway_config.model_provider = std::env::var("MANTA_MODEL_PROVIDER")
            .unwrap_or_else(|_| gateway_config.model_provider.clone());

        // Enable features based on environment variables
        // Vector Memory - enabled by default with local GGUF embeddings
        if std::env::var("MANTA_VECTOR_MEMORY_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
        {
            gateway_config.vector_memory.enabled = true;
            gateway_config.vector_memory.embedding_api_key =
                std::env::var("MANTA_EMBEDDING_API_KEY").ok();
            if let Ok(model) = std::env::var("MANTA_EMBEDDING_MODEL") {
                gateway_config.vector_memory.embedding_model = model;
            }
            println!("📊 Vector memory enabled");
        }

        // Plugins - enabled by default, disable if explicitly set to false
        if std::env::var("MANTA_PLUGINS_ENABLED")
            .map(|v| v == "false" || v == "0")
            .unwrap_or(false)
        {
            gateway_config.plugins.enabled = false;
            println!("🔌 Plugins disabled via environment");
        } else {
            println!("🔌 Plugins enabled (auto-load: {})", gateway_config.plugins.auto_load);
        }

        // Hot Reload - enabled by default, disable if explicitly set to false
        if std::env::var("MANTA_HOT_RELOAD_ENABLED")
            .map(|v| v == "false" || v == "0")
            .unwrap_or(false)
        {
            gateway_config.hot_reload.enabled = false;
            println!("♻️  Hot reload disabled via environment");
        } else {
            println!("♻️  Hot reload enabled");
        }

        // ACP - enabled by default, disable if explicitly set to false
        if std::env::var("MANTA_ACP_ENABLED")
            .map(|v| v == "false" || v == "0")
            .unwrap_or(false)
        {
            gateway_config.acp.enabled = false;
            println!("🎛️  ACP disabled via environment");
        } else {
            println!("🎛️  ACP enabled");
        }

        // Configure LLM Provider from environment variables (legacy support)
        if let (Ok(base_url), Ok(api_key)) =
            (std::env::var("MANTA_BASE_URL"), std::env::var("MANTA_API_KEY"))
        {
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
                api_keys: Vec::new(),
                auth_profile: None,
                oauth: None,
                base_url: Some(base_url),
                timeout: std::time::Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };

            let provider_name = std::env::var("MANTA_MODEL_PROVIDER")
                .unwrap_or_else(|_| if is_anthropic { "anthropic".to_string() } else { "openai".to_string() });
            gateway_config
                .providers
                .insert(provider_name.clone(), provider_config);
            println!("🤖 Configured {} provider from environment", provider_name);
        } else if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
            // Also support direct ANTHROPIC_API_KEY
            let provider_config = crate::model_router::ProviderConfig {
                provider_type: crate::model_router::ProviderType::Anthropic,
                api_key,
                api_keys: Vec::new(),
                auth_profile: None,
                oauth: None,
                base_url: None,
                timeout: std::time::Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };
            gateway_config
                .providers
                .insert("anthropic".to_string(), provider_config);
            println!("🤖 Configured Anthropic provider from ANTHROPIC_API_KEY");
        } else if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
            // Support OPENAI_API_KEY
            let provider_config = crate::model_router::ProviderConfig {
                provider_type: crate::model_router::ProviderType::OpenAi,
                api_key,
                api_keys: Vec::new(),
                auth_profile: None,
                oauth: None,
                base_url: None,
                timeout: std::time::Duration::from_secs(60),
                max_retries: 3,
                retry_delay_ms: 1000,
            };
            gateway_config
                .providers
                .insert("openai".to_string(), provider_config);
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
        let gateway = Gateway::new(gateway_config.clone(), Some(config_path.clone())).await?;

        println!("✅ Gateway ready");
        println!("   URL: http://{}:{}", gateway_config.host, gateway_config.port);

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

                        kill(Pid::from_raw(pid as i32), Signal::SIGTERM).map_err(|e| {
                            crate::error::MantaError::Internal(format!(
                                "Failed to send signal: {}",
                                e
                            ))
                        })?;
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

                        kill(Pid::from_raw(pid as i32), Signal::SIGKILL).map_err(|e| {
                            crate::error::MantaError::Internal(format!(
                                "Failed to send signal: {}",
                                e
                            ))
                        })?;
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
                    println!("   Port: {}", self.config.port);
                    println!("   URL: http://{}:{}", self.config.host, self.config.port);
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
                .map_err(crate::error::MantaError::Io)?;
        }

        tokio::fs::write(&self.config.pid_file, pid.to_string())
            .await
            .map_err(crate::error::MantaError::Io)?;

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

    #[allow(dead_code)]
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

        let db_url = format!("sqlite:///{}", db_path.display());

        println!("💾 Memory store: {}", db_path.display());

        crate::memory::SqliteMemoryStore::new(&db_url).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daemon_config_creation() {
        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            pid_file: PathBuf::from("/tmp/manta.pid"),
        };
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 8080);
        assert_eq!(config.pid_file, PathBuf::from("/tmp/manta.pid"));
    }

    #[test]
    fn test_daemon_manager_new() {
        let config = DaemonConfig {
            host: "0.0.0.0".to_string(),
            port: 3000,
            pid_file: PathBuf::from("/tmp/manta-test.pid"),
        };
        let manager = DaemonManager::new(config.clone());
        assert!(manager.is_ok());
        let manager = manager.unwrap();
        assert_eq!(manager.config.host, "0.0.0.0");
        assert_eq!(manager.config.port, 3000);
    }

    #[tokio::test]
    async fn test_write_and_read_pid() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pid_file = temp_dir.path().join("test.pid");

        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            pid_file: pid_file.clone(),
        };
        let manager = DaemonManager::new(config).unwrap();

        // Write a PID
        manager.write_pid(12345).await.unwrap();
        assert!(pid_file.exists());

        // Read it back
        let pid = manager.read_pid().await;
        assert_eq!(pid, Some(12345));
    }

    #[tokio::test]
    async fn test_read_pid_missing_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pid_file = temp_dir.path().join("nonexistent.pid");

        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            pid_file,
        };
        let manager = DaemonManager::new(config).unwrap();

        let pid = manager.read_pid().await;
        assert_eq!(pid, None);
    }

    #[tokio::test]
    async fn test_read_pid_invalid_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pid_file = temp_dir.path().join("invalid.pid");

        tokio::fs::write(&pid_file, "not-a-number").await.unwrap();

        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            pid_file,
        };
        let manager = DaemonManager::new(config).unwrap();

        let pid = manager.read_pid().await;
        assert_eq!(pid, None);
    }

    #[tokio::test]
    async fn test_write_pid_creates_parent_dirs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pid_file = temp_dir.path().join("nested").join("dirs").join("test.pid");

        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            pid_file: pid_file.clone(),
        };
        let manager = DaemonManager::new(config).unwrap();

        manager.write_pid(99999).await.unwrap();
        assert!(pid_file.exists());

        let content = tokio::fs::read_to_string(&pid_file).await.unwrap();
        assert_eq!(content, "99999");
    }

    #[tokio::test]
    async fn test_is_process_running_self() {
        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            pid_file: PathBuf::from("/tmp/manta.pid"),
        };
        let manager = DaemonManager::new(config).unwrap();

        let current_pid = std::process::id();
        assert!(manager.is_process_running(current_pid).await);
    }

    #[tokio::test]
    async fn test_is_process_running_fake_pid() {
        let config = DaemonConfig {
            host: "127.0.0.1".to_string(),
            port: 8080,
            pid_file: PathBuf::from("/tmp/manta.pid"),
        };
        let manager = DaemonManager::new(config).unwrap();

        // PID 999999 is extremely unlikely to exist
        assert!(!manager.is_process_running(999999).await);
    }
}
