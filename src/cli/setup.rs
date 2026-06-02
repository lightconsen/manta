//! Setup wizard for Manta
//!
//! Interactive CLI wizard to configure ~/.manta/manta.toml.

use crate::error::Result;
use std::io::{self, Write};

/// Check if user wants to quit the wizard.
fn should_quit(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.eq_ignore_ascii_case("q") || trimmed.eq_ignore_ascii_case("quit")
}

/// Read a line from stdin, returning `Ok(None)` if the user wants to quit.
fn read_line(prompt: &str) -> Result<Option<String>> {
    print!("{}", prompt);
    io::stdout().flush().unwrap();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).unwrap();
    if should_quit(&buf) {
        return Ok(None);
    }
    Ok(Some(buf))
}

/// Run the interactive setup wizard.
/// Reads existing ~/.manta/manta.toml if present and allows editing.
pub async fn run_setup() -> Result<()> {
    println!("🐙 Manta Setup Wizard");
    println!("=====================");
    println!("   Type 'q' or 'quit' at any prompt to exit without saving.\n");

    let manta_dir = crate::dirs::manta_dir();
    let config_path = manta_dir.join("manta.toml");

    // Ensure directory exists
    if !manta_dir.exists() {
        println!("📁 Creating Manta directory at {:?}...", manta_dir);
        tokio::fs::create_dir_all(&manta_dir)
            .await
            .map_err(crate::error::MantaError::Io)?;
    }

    // Load existing config or use defaults
    let mut config = if config_path.exists() {
        println!("📄 Found existing config at {:?}", config_path);
        match tokio::fs::read_to_string(&config_path).await {
            Ok(content) => match toml::from_str::<crate::gateway::GatewayConfig>(&content) {
                Ok(c) => {
                    println!("   Loaded existing configuration.\n");
                    c
                }
                Err(e) => {
                    println!("   ⚠️  Failed to parse existing config: {}", e);
                    println!("   Starting with defaults.\n");
                    crate::gateway::GatewayConfig::default()
                }
            },
            Err(e) => {
                println!("   ⚠️  Failed to read existing config: {}", e);
                crate::gateway::GatewayConfig::default()
            }
        }
    } else {
        println!("📄 No existing config found. Starting fresh.\n");
        crate::gateway::GatewayConfig::default()
    };

    // ── 1. LLM Provider ───────────────────────────────────────────────
    let presets = crate::model_router::provider_presets();
    let mut preset_names: Vec<_> = presets.keys().cloned().collect();
    preset_names.sort();

    println!("1. LLM Provider");
    for (i, name) in preset_names.iter().enumerate() {
        if let Some(p) = presets.get(name) {
            println!("   {}. {} - {}", i + 1, p.display_name, name);
        }
    }
    println!("   {}. Other (custom)", preset_names.len() + 1);

    let default_provider = config.model_provider.clone();
    let input = match read_line(&format!("   Select provider [{}]: ", default_provider))? {
        Some(v) => v,
        None => {
            println!("\n❌ Setup cancelled. No changes saved.");
            return Ok(());
        }
    };
    let input = input.trim();

    let (provider_name, preset) = if input.is_empty() {
        (default_provider.clone(), presets.get(&default_provider).cloned())
    } else if let Ok(idx) = input.parse::<usize>() {
        if idx > 0 && idx <= preset_names.len() {
            let name = preset_names[idx - 1].clone();
            (name.clone(), presets.get(&name).cloned())
        } else {
            println!("   Custom provider selected.");
            ("custom".to_string(), None)
        }
    } else if presets.contains_key(input) {
        (input.to_string(), presets.get(input).cloned())
    } else {
        (input.to_string(), None)
    };

    // ── 2. API Key ────────────────────────────────────────────────────
    let existing_key = config
        .providers
        .get(&provider_name)
        .map(|p| p.api_key.clone())
        .unwrap_or_default();

    let prompt = {
        let mut s = "\n2. API Key".to_string();
        if !existing_key.is_empty() {
            let masked = if existing_key.len() > 8 {
                format!("{}...{}", &existing_key[..4], &existing_key[existing_key.len() - 4..])
            } else {
                "***".to_string()
            };
            s.push_str(&format!(" [{}]", masked));
        }
        s.push_str(" (leave empty to keep existing): ");
        s
    };
    let api_key_input = match read_line(&prompt)? {
        Some(v) => v,
        None => {
            println!("\n❌ Setup cancelled. No changes saved.");
            return Ok(());
        }
    };
    let api_key = api_key_input.trim();
    let api_key = if api_key.is_empty() {
        if existing_key.is_empty() {
            println!("   ⚠️  No API key provided. You can set it later via MANTA_API_KEY env var.");
            String::new()
        } else {
            existing_key
        }
    } else {
        api_key.to_string()
    };

    // ── 3. Base URL ───────────────────────────────────────────────────
    let default_base_url = preset
        .as_ref()
        .and_then(|p| p.default_base_url.clone())
        .or_else(|| {
            config
                .providers
                .get(&provider_name)
                .and_then(|p| p.base_url.clone())
        });

    let prompt = {
        let mut s = "\n3. Base URL".to_string();
        if let Some(ref url) = default_base_url {
            s.push_str(&format!(" [{}]", url));
        }
        s.push_str(" (leave empty for default): ");
        s
    };
    let base_url_input = match read_line(&prompt)? {
        Some(v) => v,
        None => {
            println!("\n❌ Setup cancelled. No changes saved.");
            return Ok(());
        }
    };
    let base_url_input = base_url_input.trim();
    let base_url = if base_url_input.is_empty() {
        default_base_url
    } else {
        Some(base_url_input.to_string())
    };

    // ── 4. Model ──────────────────────────────────────────────────────
    let suggested_models = preset.as_ref().map(|p| p.models.clone()).unwrap_or_default();
    let default_model = config.model.clone();

    println!("\n4. Model");
    if !suggested_models.is_empty() {
        for (i, m) in suggested_models.iter().enumerate() {
            let marker = if *m == default_model { " (current)" } else { "" };
            println!("   {}. {}{}", i + 1, m, marker);
        }
        println!("   {}. Other", suggested_models.len() + 1);
    }
    let model_input = match read_line(&format!("   Select model [{}]: ", default_model))? {
        Some(v) => v,
        None => {
            println!("\n❌ Setup cancelled. No changes saved.");
            return Ok(());
        }
    };
    let model_input = model_input.trim();

    let model = if model_input.is_empty() {
        default_model
    } else if let Ok(idx) = model_input.parse::<usize>() {
        if idx > 0 && idx <= suggested_models.len() {
            suggested_models[idx - 1].clone()
        } else {
            let custom = match read_line("   Enter model name: ")? {
                Some(v) => v,
                None => {
                    println!("\n❌ Setup cancelled. No changes saved.");
                    return Ok(());
                }
            };
            custom.trim().to_string()
        }
    } else {
        model_input.to_string()
    };

    // ── Build provider config ─────────────────────────────────────────
    let provider_type = preset
        .as_ref()
        .map(|p| p.protocol.clone())
        .unwrap_or_else(|| crate::model_router::ProviderType::OpenAi);

    let provider_config = crate::model_router::ProviderConfig {
        provider_type,
        api_key: api_key.clone(),
        api_keys: vec![],
        auth_profile: None,
        oauth: None,
        base_url: base_url.clone(),
        timeout: std::time::Duration::from_secs(30),
        max_retries: 3,
        retry_delay_ms: 1000,
    };

    config.providers.insert(provider_name.clone(), provider_config);
    config.model = model;
    config.model_provider = provider_name;

    // ── 5. Server Host/Port (optional) ────────────────────────────────
    println!("\n5. Server Settings");
    let host_input = match read_line(&format!("   Host [{}]: ", config.host))? {
        Some(v) => v,
        None => {
            println!("\n❌ Setup cancelled. No changes saved.");
            return Ok(());
        }
    };
    let host_input = host_input.trim();
    if !host_input.is_empty() {
        config.host = host_input.to_string();
    }

    let port_input = match read_line(&format!("   Port [{}]: ", config.port))? {
        Some(v) => v,
        None => {
            println!("\n❌ Setup cancelled. No changes saved.");
            return Ok(());
        }
    };
    let port_input = port_input.trim();
    if !port_input.is_empty() {
        if let Ok(p) = port_input.parse::<u16>() {
            config.port = p;
        }
    }

    // ── Write config ──────────────────────────────────────────────────
    let toml_str = toml::to_string_pretty(&config).map_err(|e| {
        crate::error::MantaError::Validation(format!("Failed to serialize config: {}", e))
    })?;

    tokio::fs::write(&config_path, toml_str)
        .await
        .map_err(crate::error::MantaError::Io)?;

    println!("\n✅ Configuration saved to {:?}", config_path);
    println!("\nNext steps:");
    println!("  1. Start the daemon:  ./manta start");
    println!("  2. Open Web UI:       http://{}:{}", config.host, config.port);
    println!("  3. Edit config:       {:?}", config_path);

    Ok(())
}
