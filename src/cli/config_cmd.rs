//! Configuration management commands for Manta
//!
//! Get, set, and validate configuration values directly.

use crate::error::Result;
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum ConfigCommands {
    /// Show the current configuration
    Show {
        /// Output format
        #[arg(short, long, value_enum, default_value = "toml")]
        format: super::ConfigFormat,
    },
    /// Get a specific configuration value by key (dot-notation, e.g., gateway.host)
    Get {
        /// Configuration key
        key: String,
    },
    /// Set a configuration value by key (format: key=value)
    Set {
        /// Configuration key=value pair
        key_value: String,
        /// Path to configuration file
        #[arg(short, long)]
        file: Option<PathBuf>,
    },
    /// Validate the current configuration
    Validate,
}

/// Run config commands
pub async fn run_config_command(command: &ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::Show { format } => show_config(format).await,
        ConfigCommands::Get { key } => get_config_value(key).await,
        ConfigCommands::Set { key_value, file } => set_config_value(key_value, file.as_ref()).await,
        ConfigCommands::Validate => validate_config().await,
    }
}

async fn show_config(format: &super::ConfigFormat) -> Result<()> {
    use crate::config::Config;

    let config = Config::load()?;

    match format {
        super::ConfigFormat::Toml => {
            println!("# Manta Configuration");
            println!("# Config file: {:?}", crate::dirs::manta_dir().join("manta.toml"));
            println!();
            println!("{:#?}", config);
        }
        super::ConfigFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&config).unwrap_or_default());
        }
        super::ConfigFormat::Yaml => {
            println!("{}", serde_yaml::to_string(&config).unwrap_or_default());
        }
    }

    Ok(())
}

async fn get_config_value(key: &str) -> Result<()> {
    use crate::config::Config;

    let config = Config::load()?;
    let config_json = serde_json::to_value(&config)?;

    let mut current = &config_json;
    for part in key.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                if let Some(v) = map.get(part) {
                    current = v;
                } else {
                    eprintln!("Key '{}' not found in configuration", key);
                    return Ok(());
                }
            }
            serde_json::Value::Array(arr) => {
                if let Ok(index) = part.parse::<usize>() {
                    if let Some(v) = arr.get(index) {
                        current = v;
                    } else {
                        eprintln!("Index {} out of bounds for key '{}'", index, key);
                        return Ok(());
                    }
                } else {
                    eprintln!("Expected array index for key '{}'", key);
                    return Ok(());
                }
            }
            _ => {
                eprintln!("Key '{}' not found in configuration", key);
                return Ok(());
            }
        }
    }

    println!("{}", serde_json::to_string_pretty(current).unwrap_or_default());
    Ok(())
}

async fn set_config_value(key_value: &str, file: Option<&PathBuf>) -> Result<()> {
    let parts: Vec<&str> = key_value.splitn(2, '=').collect();
    if parts.len() != 2 {
        eprintln!("Invalid format. Use: key=value");
        return Ok(());
    }

    let key = parts[0];
    let value = parts[1];

    let config_path = file
        .cloned()
        .unwrap_or_else(|| crate::dirs::manta_dir().join("manta.toml"));

    if !config_path.exists() {
        eprintln!("Configuration file not found at {:?}", config_path);
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;

    // Simple TOML key replacement
    let mut updated = false;
    let lines: Vec<String> = content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with(&format!("{} =", key))
                || trimmed.starts_with(&format!("{}=", key))
            {
                updated = true;
                format!("{} = \"{}\"", key, value)
            } else {
                line.to_string()
            }
        })
        .collect();

    let new_content = if updated {
        lines.join("\n")
    } else {
        // Append to end
        format!("{}\n{} = \"{}\"\n", content.trim_end(), key, value)
    };

    std::fs::write(&config_path, new_content)?;
    println!("✅ Set {} = {} in {:?}", key, value, config_path);

    Ok(())
}

async fn validate_config() -> Result<()> {
    use crate::config::Config;

    match Config::load() {
        Ok(_config) => {
            println!("✅ Configuration is valid");
            Ok(())
        }
        Err(e) => {
            eprintln!("❌ Configuration error: {}", e);
            Err(e)
        }
    }
}
