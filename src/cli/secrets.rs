//! `syscity secrets` — inspect, migrate, and purge the secret store.
//!
//! These commands operate on the tiered secret store (OS keyring preferred,
//! 0600 file fallback) and never print secret values.

use std::collections::BTreeSet;

use clap::Subcommand;
use tracing::{info, warn};

use crate::error::{Result, SyscityError};
use crate::gateway::GatewayConfig;
use crate::secrets::{
    probe_keyring, route_store, FileStore, KeyringStore, SecretId, SecretOrigin, SecretStore,
    SENSITIVE_CHANNEL_CREDENTIALS,
};

/// Namespaces handled by the secret store and their human-readable label.
const NAMESPACES: &[(&str, &str)] = &[
    ("llm", "LLM provider API keys"),
    ("mcp-env", "MCP server environment tokens"),
    ("mcp-oauth", "MCP OAuth refresh tokens"),
    ("channel", "Channel credentials"),
    ("webhook", "Webhook secrets"),
    ("security", "Security / OAuth client secrets"),
    ("plugin", "Plugin secret keys"),
];

/// Subcommands for secret store management.
#[derive(Debug, Subcommand)]
pub enum SecretsCommands {
    /// List stored secrets — names and locations only, never values
    List,
    /// Migrate legacy plaintext secrets (config, mcp_env, OAuth) into the store
    Migrate,
    /// Purge all secrets in a namespace
    Purge {
        /// Namespace to purge (llm, mcp-env, mcp-oauth, channel, webhook, security, plugin)
        namespace: String,
    },
}

/// Run a secrets subcommand.
pub async fn run_secrets_command(command: &SecretsCommands) -> Result<()> {
    match command {
        SecretsCommands::List => run_secrets_list().await,
        SecretsCommands::Migrate => run_secrets_migrate().await,
        SecretsCommands::Purge { namespace } => run_secrets_purge(namespace).await,
    }
}

/// Load the gateway config from the default location, if present.
///
/// Like the channel CLI, this uses the default config file rather than the
/// `--config` override for consistency with sibling subcommands.
async fn load_gateway_config() -> Option<GatewayConfig> {
    let path = crate::dirs::default_config_file();
    let content = tokio::fs::read_to_string(&path).await.ok()?;
    toml::from_str(&content).ok()
}

/// Enumerate entities of a namespace that exist on disk (file backend).
fn file_entities(namespace: &str) -> Vec<String> {
    let dir = crate::secrets::secrets_root_dir().join(namespace);
    let mut entities = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if let Some(stem) = name.to_str().and_then(|s| s.strip_suffix(".toml")) {
                entities.push(stem.to_string());
            }
        }
    }
    entities.sort();
    entities
}

/// The OAuth providers understood by the gateway (`github`, `google`).
fn oauth_provider_configs(
    config: &GatewayConfig,
) -> [(&'static str, &Option<crate::gateway::auth::OAuthProviderConfig>); 2] {
    [
        ("github", &config.security.oauth.github),
        ("google", &config.security.oauth.google),
    ]
}

/// Gather every known `(namespace, entity)` pair — from the on-disk file
/// backend and from channels / OAuth providers referenced in the gateway
/// config (which may live in the keyring and have no file).
async fn collect_pairs() -> BTreeSet<(String, String)> {
    let mut pairs = BTreeSet::new();
    for (namespace, _label) in NAMESPACES {
        for entity in file_entities(namespace) {
            pairs.insert((namespace.to_string(), entity));
        }
    }
    if let Some(config) = load_gateway_config().await {
        for id in config.channels.keys() {
            pairs.insert(("channel".to_string(), id.clone()));
        }
        for (provider, cfg) in oauth_provider_configs(&config) {
            if let Some(cfg) = cfg {
                if !cfg.client_secret.is_empty() {
                    pairs.insert(("security".to_string(), format!("oauth-{provider}")));
                }
            }
        }
    }
    pairs
}

/// `syscity secrets list` — show names and locations, never values.
async fn run_secrets_list() -> Result<()> {
    let keyring = probe_keyring();
    println!(
        "Secret store backend: {}",
        if keyring {
            "OS keyring (Tier 1, 0600 file fallback)"
        } else {
            "0600 file (Tier 2)"
        }
    );

    let pairs = collect_pairs().await;
    if pairs.is_empty() {
        println!("No secrets stored.");
        return Ok(());
    }

    println!();
    println!("{:<10} {:<22} {:<20} {:<5}", "Namespace", "Entity", "Location", "Stored");
    for (namespace, entity) in pairs {
        let in_keyring = KeyringStore::new(&namespace).has_entity(&entity).await;
        let in_file = FileStore::new(&namespace).has_entity(&entity).await;
        let location = match (in_keyring, in_file) {
            (true, true) => "keyring + file",
            (true, false) => "keyring",
            (false, true) => "file",
            (false, false) => "absent",
        };
        let stored = if in_keyring || in_file { "yes" } else { "no" };
        println!("{:<10} {:<22} {:<20} {}", namespace, entity, location, stored);
    }
    println!();
    println!("Values are never printed. Run `syscity secrets migrate` to move");
    println!("plaintext config values into the secret store.");
    Ok(())
}

/// `syscity secrets migrate` — mirror plaintext config secrets into the store
/// and strip the plaintext copies from `config.toml`.
async fn run_secrets_migrate() -> Result<()> {
    // 1. Move legacy `~/.syscity/mcp_env/` files into the store (idempotent).
    crate::secrets::migrate_legacy_mcp_env().await?;
    info!("Migrated legacy mcp_env files");

    // 2. Sweep legacy `mcp_tokens` sidecars that still carry plaintext token
    //    fields into the store and rewrite them metadata-only (idempotent).
    crate::mcp::migrate_legacy_mcp_tokens().await?;
    info!("Migrated legacy mcp_tokens files");

    let config_path = crate::dirs::default_config_file();
    if !config_path.exists() {
        println!("No config file at {}; nothing else to migrate.", config_path.display());
        return Ok(());
    }
    let content = tokio::fs::read_to_string(&config_path).await?;
    let mut config: GatewayConfig = toml::from_str(&content)?;

    let mut stored = 0usize;
    let mut changed = false;

    // 3. Mirror channel credentials into the store (namespace `channel`).
    for (id, channel) in &config.channels {
        for kind in SENSITIVE_CHANNEL_CREDENTIALS {
            if channel
                .credentials
                .get(*kind)
                .is_some_and(|v| !v.is_empty())
            {
                stored += 1;
            }
        }
        crate::secrets::persist_channel_secrets(id, &channel.credentials).await?;
    }

    // 4. Mirror OAuth client secrets into the store (namespace `security`).
    for (provider, cfg) in oauth_provider_configs(&config) {
        if let Some(cfg) = cfg {
            if !cfg.client_secret.is_empty() {
                route_store("security")
                    .set(
                        &SecretId::new("security", &format!("oauth-{provider}"), "client_secret"),
                        &cfg.client_secret,
                        SecretOrigin::UserEntered,
                    )
                    .await?;
                stored += 1;
            }
        }
    }

    // 5. Advisory for shared_token (kept in config; env reference preferred).
    if config
        .security
        .shared_token
        .as_deref()
        .is_some_and(|s| !s.is_empty())
    {
        warn!(
            "security.shared_token is in plaintext config; prefer the \
             SYSCITY_SECURITY_SHARED_TOKEN environment variable"
        );
    }

    // 6. Strip the now-stored plaintext copies from config (channels + OAuth).
    for channel in config.channels.values_mut() {
        for kind in SENSITIVE_CHANNEL_CREDENTIALS {
            if channel.credentials.remove(*kind).is_some() {
                changed = true;
            }
        }
    }
    for cfg in [
        config.security.oauth.github.as_mut(),
        config.security.oauth.google.as_mut(),
    ]
    .into_iter()
    .flatten()
    {
        if !cfg.client_secret.is_empty() {
            cfg.client_secret.clear();
            changed = true;
        }
    }

    if changed {
        let config_str = toml::to_string_pretty(&config)?;
        tokio::fs::write(&config_path, config_str).await?;
    }

    println!(
        "Migrated {stored} secret value(s) into the secret store; config.toml plaintext stripped."
    );
    Ok(())
}

/// `syscity secrets purge {namespace}` — delete every stored secret in a
/// namespace.
async fn run_secrets_purge(namespace: &str) -> Result<()> {
    let valid = NAMESPACES.iter().any(|(ns, _)| *ns == namespace);
    if !valid {
        return Err(SyscityError::Validation(format!(
            "unknown secrets namespace '{namespace}' (expected one of: {})",
            NAMESPACES
                .iter()
                .map(|(ns, _)| *ns)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let store = route_store(namespace);

    // Keyring entries cannot be enumerated, so include config-derived entities
    // that may live there in addition to the on-disk file entities.
    let mut entities = file_entities(namespace);
    if let Some(config) = load_gateway_config().await {
        match namespace {
            "channel" => entities.extend(config.channels.keys().cloned()),
            "security" => {
                for (provider, cfg) in oauth_provider_configs(&config) {
                    if let Some(cfg) = cfg {
                        if !cfg.client_secret.is_empty() {
                            entities.push(format!("oauth-{provider}"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    entities.sort();
    entities.dedup();

    let mut purged = 0usize;
    for entity in entities {
        if store.has_entity(&entity).await {
            store.delete_entity(&entity).await?;
            purged += 1;
            println!("Purged {namespace}/{entity}");
        }
    }

    if purged == 0 {
        println!("No secrets found in namespace '{namespace}'.");
    } else {
        println!("Purged {purged} entit(ies) from namespace '{namespace}'.");
    }
    Ok(())
}
