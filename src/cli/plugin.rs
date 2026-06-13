//! Plugin management commands for Syscity

use crate::error::{Result, SyscityError};
use clap::Subcommand;
use std::path::PathBuf;

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum PluginCommands {
    /// List all plugins
    List {
        /// Show loaded plugins only
        #[arg(short, long)]
        loaded: bool,
        /// Show verbose information
        #[arg(short, long)]
        verbose: bool,
    },
    /// Install a plugin from a directory (copies it to the plugins folder)
    Install {
        /// Path to plugin directory containing plugin.json
        path: PathBuf,
        /// Plugin name (optional, defaults to directory name)
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin ID
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Enable a plugin
    Enable {
        /// Plugin ID
        name: String,
    },
    /// Disable a plugin
    Disable {
        /// Plugin ID
        name: String,
    },
    /// Show plugin information
    Info {
        /// Plugin ID
        name: String,
    },
    /// Reload plugins (lists current state; full reload requires daemon restart)
    Reload,
    /// Install a plugin from a remote registry
    RegistryInstall {
        /// Plugin name or ID
        name: String,
        /// Registry URL (defaults to https://plugins.syscity.dev)
        #[arg(short, long)]
        registry: Option<String>,
    },
    /// Search for plugins in the registry
    Search {
        /// Search query
        query: String,
        /// Registry URL (defaults to https://plugins.syscity.dev)
        #[arg(short, long)]
        registry: Option<String>,
    },
    /// Sign a plugin manifest with an ed25519 key
    Sign {
        /// Plugin directory name (within plugins directory)
        name: String,
        /// Path to file containing base64-encoded ed25519 secret key
        #[arg(short, long)]
        key_file: PathBuf,
    },
}

/// Run plugin commands
pub async fn run_plugin_command(command: &PluginCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        PluginCommands::List { loaded: _, verbose } => {
            let url = format!("{}/api/v1/plugins", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    if *verbose {
                        println!("{}", body);
                    } else if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                        let empty = vec![];
                        let plugins = json["plugins"].as_array().unwrap_or(&empty);
                        println!("Plugins ({}):", plugins.len());
                        for p in plugins {
                            let id = p["id"].as_str().unwrap_or("?");
                            let name = p["name"].as_str().unwrap_or("?");
                            let enabled = p["enabled"].as_bool().unwrap_or(false);
                            let status = if enabled { "enabled" } else { "disabled" };
                            println!("  {} ({}) [{}]", name, id, status);
                        }
                    } else {
                        println!("{}", body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon at {}: {}", DAEMON_URL, e);
                    eprintln!("Is the daemon running? Try: syscity start");
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::Install { path, name } => {
            // Resolve destination inside the Syscity plugins directory.
            let plugins_dir = crate::dirs::config_dir().join("plugins");
            tokio::fs::create_dir_all(&plugins_dir)
                .await
                .map_err(|e| SyscityError::Internal(format!("Cannot create plugins dir: {}", e)))?;

            let dest_name = name
                .clone()
                .or_else(|| {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "plugin".to_string());

            let dest = plugins_dir.join(&dest_name);

            // Copy the plugin directory (or single file) into the plugins folder.
            if path.is_dir() {
                copy_dir_all(path, &dest).await?;
            } else {
                tokio::fs::copy(path, &dest)
                    .await
                    .map_err(|e| SyscityError::Internal(format!("Copy failed: {}", e)))?;
            }

            println!("Plugin installed to {:?}", dest);
            println!("Restart the daemon (syscity restart) to load it.");
        }

        PluginCommands::Uninstall { name, force } => {
            if !force {
                println!("Uninstall plugin '{}'? Use --force to confirm.", name);
                return Ok(());
            }
            let url = format!("{}/api/v1/plugins/{}/unload", DAEMON_URL, name);
            match client.delete(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        println!("Plugin '{}' unloaded from daemon.", name);
                        println!(
                            "To remove files, delete {:?}",
                            crate::dirs::config_dir().join("plugins").join(name)
                        );
                    } else {
                        let text = resp.text().await.unwrap_or_default();
                        eprintln!("Failed to unload plugin ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::Enable { name } => {
            let url = format!("{}/api/v1/plugins/{}/enable", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Plugin '{}' enabled.", name);
                    } else {
                        eprintln!("Failed to enable plugin ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::Disable { name } => {
            let url = format!("{}/api/v1/plugins/{}/disable", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Plugin '{}' disabled.", name);
                    } else {
                        eprintln!("Failed to disable plugin ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::Info { name } => {
            // Retrieve full list and filter by id/name.
            let url = format!("{}/api/v1/plugins", DAEMON_URL);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                        let empty = vec![];
                        let plugins = json["plugins"].as_array().unwrap_or(&empty);
                        let found = plugins.iter().find(|p| {
                            p["id"].as_str() == Some(name.as_str())
                                || p["name"].as_str() == Some(name.as_str())
                        });
                        match found {
                            Some(p) => {
                                println!("{}", serde_json::to_string_pretty(p).unwrap_or_default())
                            }
                            None => {
                                eprintln!("Plugin '{}' not found", name);
                            }
                        }
                    } else {
                        println!("{}", body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::Reload => {
            let url = format!("{}/api/v1/plugins/reload", DAEMON_URL);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Plugins reloaded successfully.");
                        if !text.is_empty() {
                            println!("{}", text);
                        }
                    } else {
                        eprintln!("Reload failed ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::RegistryInstall { name, registry } => {
            let url = format!("{}/api/v1/plugins/install", DAEMON_URL);
            let body = serde_json::json!({
                "name": name,
                "registry": registry,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Plugin '{}' installed successfully.", name);
                        if !text.is_empty() && text != "null" {
                            println!("{}", text);
                        }
                    } else {
                        eprintln!("Failed to install plugin ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::Search { query, registry } => {
            let registry_param = registry
                .as_ref()
                .map(|r| format!("&registry={}", r))
                .unwrap_or_default();
            let url = format!("{}/api/v1/plugins/search?q={}{}", DAEMON_URL, query, registry_param);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            let empty = vec![];
                            let plugins = json["results"].as_array().unwrap_or(&empty);
                            println!("Search results for '{}' ({}):", query, plugins.len());
                            for p in plugins {
                                let id = p["id"].as_str().unwrap_or("?");
                                let name = p["name"].as_str().unwrap_or("?");
                                let version = p["version"].as_str().unwrap_or("?");
                                let desc = p["description"].as_str().unwrap_or("");
                                println!("  {} ({}) v{} - {}", name, id, version, desc);
                            }
                        } else {
                            println!("{}", text);
                        }
                    } else {
                        eprintln!("Search failed ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }

        PluginCommands::Sign { name, key_file } => {
            let url = format!("{}/api/v1/plugins/sign", DAEMON_URL);
            let key_content = match tokio::fs::read_to_string(key_file).await {
                Ok(c) => c.trim().to_string(),
                Err(e) => {
                    eprintln!("Failed to read key file {:?}: {}", key_file, e);
                    return Err(SyscityError::Internal(format!(
                        "Cannot read key file {:?}: {}",
                        key_file, e
                    )));
                }
            };
            let body = serde_json::json!({
                "name": name,
                "secret_key": key_content,
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Plugin '{}' signed successfully.", name);
                        if !text.is_empty() && text != "null" {
                            println!("{}", text);
                        }
                    } else {
                        eprintln!("Failed to sign plugin ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
    }

    Ok(())
}

/// Recursively copy a directory tree.
async fn copy_dir_all(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    tokio::fs::create_dir_all(dst)
        .await
        .map_err(|e| SyscityError::Internal(format!("mkdir {:?}: {}", dst, e)))?;

    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| SyscityError::Internal(format!("read_dir {:?}: {}", src, e)))?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            Box::pin(copy_dir_all(&src_path, &dst_path)).await?;
        } else {
            tokio::fs::copy(&src_path, &dst_path)
                .await
                .map_err(|e| SyscityError::Internal(format!("copy {:?}: {}", src_path, e)))?;
        }
    }

    Ok(())
}
