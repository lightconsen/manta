//! Setup wizard for Manta
//!
//! Interactive CLI wizard to configure ~/.manta/manta.toml.

use crate::error::Result;
use dialoguer::{Input, Password, Select};

/// Run the interactive setup wizard.
/// Reads existing ~/.manta/manta.toml if present and allows editing.
pub async fn run_setup() -> Result<()> {
    println!("🐙 Manta Setup Wizard");
    println!("=====================");
    println!("   Use ↑/↓ arrows to select, Enter to confirm.\n");

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

    let mut provider_items: Vec<String> = preset_names
        .iter()
        .map(|name| {
            if let Some(p) = presets.get(name) {
                format!("{} - {}", p.display_name, name)
            } else {
                name.clone()
            }
        })
        .collect();
    provider_items.push("Other (custom)".to_string());
    provider_items.push("⏭  Skip this step".to_string());
    provider_items.push("❌ Cancel setup".to_string());

    let default_provider_idx = preset_names
        .iter()
        .position(|n| *n == config.model_provider)
        .unwrap_or(0);

    let provider_selection = Select::new()
        .with_prompt("1. LLM Provider")
        .items(&provider_items)
        .default(default_provider_idx)
        .interact()
        .map_err(|e| crate::error::MantaError::Internal(format!("Input error: {}", e)))?;

    let (provider_name, preset) = if provider_selection < preset_names.len() {
        let name = preset_names[provider_selection].clone();
        (name.clone(), presets.get(&name).cloned())
    } else if provider_selection == preset_names.len() {
        // "Other (custom)"
        println!("   Custom provider selected.");
        let custom: String = Input::new()
            .with_prompt("   Enter provider name")
            .interact_text()
            .map_err(|e| crate::error::MantaError::Internal(format!("Input error: {}", e)))?;
        (custom.trim().to_string(), None)
    } else if provider_selection == preset_names.len() + 1 {
        // "Skip this step" — keep existing provider
        println!("   Skipping provider selection.");
        (config.model_provider.clone(), presets.get(&config.model_provider).cloned())
    } else {
        // "Cancel setup"
        println!("\n❌ Setup cancelled. No changes saved.");
        return Ok(());
    };

    // ── 2. Model ──────────────────────────────────────────────────────
    let suggested_models = preset
        .as_ref()
        .map(|p| p.models.clone())
        .unwrap_or_default();
    let default_model = config.model.clone();

    let mut model_items: Vec<String> = suggested_models.clone();
    if !model_items.is_empty() {
        // Mark current model
        for item in &mut model_items {
            if *item == default_model {
                *item = format!("{} (current)", item);
            }
        }
        model_items.push("Other".to_string());
    }
    model_items.push("⏭  Skip this step".to_string());
    model_items.push("❌ Cancel setup".to_string());

    let default_model_idx = suggested_models
        .iter()
        .position(|m| *m == default_model)
        .unwrap_or(0);

    let model_selection = Select::new()
        .with_prompt("\n2. Model")
        .items(&model_items)
        .default(default_model_idx)
        .interact()
        .map_err(|e| crate::error::MantaError::Internal(format!("Input error: {}", e)))?;

    let model = if !suggested_models.is_empty() && model_selection < suggested_models.len() {
        suggested_models[model_selection].clone()
    } else if !suggested_models.is_empty() && model_selection == suggested_models.len() {
        // "Other"
        let custom: String = Input::new()
            .with_prompt("   Enter model name")
            .interact_text()
            .map_err(|e| crate::error::MantaError::Internal(format!("Input error: {}", e)))?;
        custom.trim().to_string()
    } else if model_selection == model_items.len() - 2 {
        // "Skip this step"
        println!("   Skipping model selection.");
        config.model.clone()
    } else if model_selection == model_items.len() - 1 {
        // "Cancel setup"
        println!("\n❌ Setup cancelled. No changes saved.");
        return Ok(());
    } else {
        // Should not reach here, but fallback
        default_model
    };

    // ── 3. API Key ────────────────────────────────────────────────────
    let existing_key = config
        .providers
        .get(&provider_name)
        .map(|p| p.api_key.clone())
        .unwrap_or_default();

    let prompt = if existing_key.is_empty() {
        format!("\n3. API Key for {} (Enter to skip)", provider_name)
    } else {
        let masked = if existing_key.len() > 8 {
            format!(
                "{}...{}",
                &existing_key[..4],
                &existing_key[existing_key.len() - 4..]
            )
        } else {
            "***".to_string()
        };
        format!("\n3. API Key for {} [{}] (Enter to keep)", provider_name, masked)
    };

    let api_key_input = Password::new()
        .with_prompt(prompt)
        .allow_empty_password(true)
        .interact()
        .map_err(|e| crate::error::MantaError::Internal(format!("Input error: {}", e)))?;

    let api_key = if api_key_input.trim().is_empty() {
        if existing_key.is_empty() {
            println!("   ⚠️  No API key provided. You can set it later via MANTA_API_KEY env var.");
            String::new()
        } else {
            existing_key
        }
    } else {
        api_key_input.trim().to_string()
    };

    // ── 4. Base URL (uses preset default, no user prompt) ─────────────
    let base_url = preset
        .as_ref()
        .and_then(|p| p.default_base_url.clone())
        .or_else(|| {
            config
                .providers
                .get(&provider_name)
                .and_then(|p| p.base_url.clone())
        });

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

    config
        .providers
        .insert(provider_name.clone(), provider_config);
    config.model = model;
    config.model_provider = provider_name;

    // ── 4. Server Host/Port (optional) ────────────────────────────────
    println!("\n4. Server Settings");

    let host_input: String = Input::new()
        .with_prompt(format!(
            "   Host [{}] (Enter to keep)",
            config.host
        ))
        .allow_empty(true)
        .interact_text()
        .map_err(|e| crate::error::MantaError::Internal(format!("Input error: {}", e)))?;

    if !host_input.trim().is_empty() {
        config.host = host_input.trim().to_string();
    }

    let port_input: String = Input::new()
        .with_prompt(format!(
            "   Port [{}] (Enter to keep)",
            config.port
        ))
        .allow_empty(true)
        .interact_text()
        .map_err(|e| crate::error::MantaError::Internal(format!("Input error: {}", e)))?;

    if !port_input.trim().is_empty() {
        if let Ok(p) = port_input.trim().parse::<u16>() {
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
