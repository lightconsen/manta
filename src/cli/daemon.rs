//! Daemon management commands for Manta

use crate::daemon::{DaemonConfig, DaemonManager};
use crate::error::Result;
use std::path::PathBuf;

/// Show configuration in specified format
pub async fn show_config(format: &crate::cli::ConfigFormat) -> Result<()> {
    use crate::config::Config;

    let config = Config::load()?;

    match format {
        crate::cli::ConfigFormat::Toml => {
            println!("# Manta Configuration");
            println!("# Config file: {:?}", crate::dirs::manta_dir().join("manta.toml"));
            println!();
            println!("{:#?}", config);
        }
        crate::cli::ConfigFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&config).unwrap_or_default()
            );
        }
        crate::cli::ConfigFormat::Yaml => {
            println!(
                "{}",
                serde_yaml::to_string(&config).unwrap_or_default()
            );
        }
    }

    Ok(())
}

/// Run health check
pub async fn run_health_check(_config: &crate::config::Config) -> Result<()> {
    println!("🏥 Health Check");
    println!("===============");

    // Check config
    println!("✅ Configuration loaded");

    // Check daemon status
    let daemon_config = DaemonConfig {
        host: "127.0.0.1".to_string(),
        port: 18080,
        web_port: 18081,
        pid_file: crate::dirs::manta_dir().join("manta.pid"),
    };
    let daemon = DaemonManager::new(daemon_config)?;
    let _ = daemon.status().await;

    Ok(())
}

/// Run as an assistant process
pub async fn run_assistant_process(_config_path: &PathBuf) -> Result<()> {
    println!("🤖 Starting assistant process...");
    println!("   Note: Assistant process mode is not yet fully implemented");
    println!("   Use 'manta start --foreground' to run the daemon instead");
    Ok(())
}

/// Start the daemon
pub async fn run_start_daemon(
    host: &str,
    port: u16,
    web_port: u16,
    foreground: bool,
    _config: &crate::config::Config,
) -> Result<()> {
    let daemon_config = DaemonConfig {
        host: host.to_string(),
        port,
        web_port,
        pid_file: crate::dirs::manta_dir().join("manta.pid"),
    };

    let daemon = DaemonManager::new(daemon_config)?;

    if foreground {
        // Run in foreground with Gateway
        daemon.run_foreground().await
    } else {
        // Start in background
        daemon.start().await
    }
}

/// Stop the daemon
pub async fn run_stop_daemon(force: bool) -> Result<()> {
    let daemon_config = DaemonConfig {
        host: "127.0.0.1".to_string(),
        port: 18080,
        web_port: 18081,
        pid_file: crate::dirs::manta_dir().join("manta.pid"),
    };

    let daemon = DaemonManager::new(daemon_config)?;

    if force {
        daemon.stop_force().await
    } else {
        daemon.stop().await
    }
}

/// Check daemon status
pub async fn run_daemon_status() -> Result<()> {
    let daemon_config = DaemonConfig {
        host: "127.0.0.1".to_string(),
        port: 18080,
        web_port: 18081,
        pid_file: crate::dirs::manta_dir().join("manta.pid"),
    };

    let daemon = DaemonManager::new(daemon_config)?;
    daemon.status().await
}

/// Show and tail daemon logs
pub async fn run_logs(lines: usize, follow: bool) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::time::{Duration, interval};

    let log_path = crate::logs::log_file_path();

    if !log_path.exists() {
        println!("No log file found at {:?}", log_path);
        return Ok(());
    }

    println!("📋 Logs from: {:?}", log_path);
    println!();

    if follow {
        // Tail mode - read last N lines then follow
        let file = tokio::fs::File::open(&log_path).await?;
        let reader = BufReader::new(file);
        let mut lines_stream = reader.lines();

        // Collect and show last N lines
        let mut all_lines: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines_stream.next_line().await {
            all_lines.push(line);
            if all_lines.len() > lines {
                all_lines.remove(0);
            }
        }

        for line in all_lines {
            println!("{}", line);
        }

        // Continue following
        println!("\n--- Following log (Ctrl+C to exit) ---\n");

        let mut interval = interval(Duration::from_millis(500));
        let mut last_pos = tokio::fs::metadata(&log_path).await?.len();

        loop {
            interval.tick().await;

            let metadata = tokio::fs::metadata(&log_path).await?;
            let new_len = metadata.len();

            if new_len > last_pos {
                let file = tokio::fs::File::open(&log_path).await?;
                let reader = BufReader::new(file);
                let mut lines_stream = reader.lines();

                // Skip to last known position
                let mut pos = 0u64;
                while pos < last_pos {
                    if let Ok(Some(line)) = lines_stream.next_line().await {
                        pos += line.len() as u64 + 1; // +1 for newline
                    } else {
                        break;
                    }
                }

                // Print new lines
                while let Ok(Some(line)) = lines_stream.next_line().await {
                    println!("{}", line);
                }

                last_pos = new_len;
            }
        }
    } else {
        // Just show last N lines
        let file = tokio::fs::File::open(&log_path).await?;
        let reader = BufReader::new(file);
        let mut lines_stream = reader.lines();

        let mut all_lines: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines_stream.next_line().await {
            all_lines.push(line);
            if all_lines.len() > lines {
                all_lines.remove(0);
            }
        }

        for line in all_lines {
            println!("{}", line);
        }

        println!("\n--- Use -f to follow logs ---");
    }

    Ok(())
}
