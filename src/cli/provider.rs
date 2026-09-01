//! Provider management commands for Syscity
//!
//! Top-level CLI for listing, enabling, disabling, and switching model
//! providers (over WebSocket).

use clap::Subcommand;
use serde_json::json;

use crate::cli::ws;
use crate::error::Result;

#[derive(Debug, Subcommand)]
pub enum ProviderCommands {
    /// List available LLM providers
    List,
    /// Show provider health status
    Health {
        /// Provider ID
        id: String,
    },
    /// Enable a provider
    Enable {
        /// Provider ID
        id: String,
    },
    /// Disable a provider
    Disable {
        /// Provider ID
        id: String,
    },
    /// Switch the default model
    Switch {
        /// Concrete model ID to switch to
        model: String,
    },
    /// Show current default model
    Default,
    /// Show provider usage statistics
    Usage {
        /// Provider ID (omit for all providers)
        id: Option<String>,
    },
    /// Authenticate a provider via OAuth 2.0 + PKCE
    Auth {
        /// Provider ID (for labeling the resulting credential)
        id: String,
        /// OAuth client ID
        #[arg(short, long)]
        client_id: String,
        /// Authorization endpoint URL
        #[arg(short = 'a', long)]
        auth_url: String,
        /// Token endpoint URL
        #[arg(short = 't', long)]
        token_url: String,
        /// Optional OAuth scope
        #[arg(short, long)]
        scope: Option<String>,
        /// Local redirect callback port (default: 18081)
        #[arg(short = 'p', long, default_value = "18081")]
        redirect_port: u16,
        /// Timeout in seconds for the callback (default: 300)
        #[arg(long, default_value = "300")]
        timeout: u64,
        /// Don't open browser automatically
        #[arg(long)]
        no_browser: bool,
    },
}

/// Run provider commands (over WebSocket).
pub async fn run_provider_command(
    command: &ProviderCommands,
    _config: &crate::config::Config,
) -> Result<()> {
    match command {
        ProviderCommands::List => {
            let payload = ws::call("providers.list", json!({})).await?;
            if let Some(providers) = payload.get("providers").and_then(|p| p.as_array()) {
                println!("Providers:");
                println!("{:<20} {:<10} {:<10} Name", "ID", "Enabled", "Healthy");
                println!("{}", "-".repeat(60));
                for p in providers {
                    println!(
                        "{:<20} {:<10} {:<10} {}",
                        p.get("id").and_then(|c| c.as_str()).unwrap_or("-"),
                        if p.get("enabled").and_then(|c| c.as_bool()).unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        },
                        if p.get("healthy").and_then(|c| c.as_bool()).unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        },
                        p.get("name").and_then(|c| c.as_str()).unwrap_or("-"),
                    );
                }
            }
            Ok(())
        }
        ProviderCommands::Health { id } => {
            let payload = ws::call("providers.health", json!({ "id": id })).await?;
            println!("{}", payload);
            Ok(())
        }
        ProviderCommands::Enable { id } => {
            match ws::call("providers.enable", json!({ "id": id })).await {
                Ok(_) => println!("✅ Enabled provider {}", id),
                Err(e) => {
                    eprintln!("Failed to enable: {}", e);
                    return Err(e);
                }
            }
            Ok(())
        }
        ProviderCommands::Disable { id } => {
            match ws::call("providers.disable", json!({ "id": id })).await {
                Ok(_) => println!("✅ Disabled provider {}", id),
                Err(e) => {
                    eprintln!("Failed to disable: {}", e);
                    return Err(e);
                }
            }
            Ok(())
        }
        ProviderCommands::Switch { model } => {
            match ws::call("providers.switch", json!({ "model": model })).await {
                Ok(_) => println!("✅ Switched default model to {}", model),
                Err(e) => {
                    eprintln!("Failed to switch: {}", e);
                    return Err(e);
                }
            }
            Ok(())
        }
        ProviderCommands::Default => {
            let payload = ws::call("models.default", json!({})).await?;
            println!("{}", payload);
            Ok(())
        }
        ProviderCommands::Usage { id } => {
            let payload = if let Some(ref provider_id) = id {
                ws::call("providers.usage", json!({ "id": provider_id })).await?
            } else {
                ws::call("providers.usage", json!({})).await?
            };
            // Try to parse as formatted usage snapshots
            let snapshots_value = payload.get("usage").cloned().unwrap_or(payload);
            if let Ok(snapshots) = serde_json::from_value::<
                Vec<crate::model_router::ProviderUsageSnapshot>,
            >(snapshots_value.clone())
            {
                let fmt_config = crate::model_router::usage_formatter::FormatConfig::default();
                if id.is_some() {
                    for snapshot in &snapshots {
                        println!(
                            "{}",
                            crate::model_router::format_provider_snapshot(snapshot, &fmt_config)
                        );
                    }
                } else {
                    println!(
                        "{}",
                        crate::model_router::format_usage_report(&snapshots, &fmt_config)
                    );
                }
            } else {
                // Fallback to pretty-printed JSON
                println!("{}", serde_json::to_string_pretty(&snapshots_value).unwrap_or_default());
            }
            Ok(())
        }
        ProviderCommands::Auth {
            id,
            client_id,
            auth_url,
            token_url,
            scope,
            redirect_port,
            timeout,
            no_browser,
        } => {
            let oauth = crate::model_router::OAuthConfig {
                client_id: client_id.clone(),
                auth_url: auth_url.clone(),
                token_url: token_url.clone(),
                scope: scope.clone(),
                client_secret: None,
                redirect_port: *redirect_port,
            };
            run_auth_command(id, &oauth, *timeout, *no_browser).await
        }
    }
}

async fn run_auth_command(
    provider_id: &str,
    oauth: &crate::model_router::OAuthConfig,
    timeout_secs: u64,
    no_browser: bool,
) -> Result<()> {
    use crate::model_router::{oauth_callback, OAuthFlow};

    let flow = OAuthFlow::new();
    let authorization_url = flow.authorization_url(oauth);

    println!("\n🔐  OAuth Authorization for '{}'\n", provider_id);
    println!("Open this URL in your browser:\n");
    println!("  {}\n", authorization_url);

    if !no_browser {
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(&authorization_url)
                .spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&authorization_url)
                .spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &authorization_url])
                .spawn();
        }
    }

    println!(
        "Waiting for callback on port {} (timeout: {}s)...\n",
        oauth.redirect_port, timeout_secs
    );

    let code =
        oauth_callback::wait_for_callback(oauth.redirect_port, timeout_secs, flow.state()).await?;

    println!("Exchanging authorization code for tokens...\n");

    let credential = flow.exchange_code(&code, oauth).await?;

    println!("✅  Authorization successful for '{}'\n", provider_id);
    println!("Credential (add to your config):\n");

    match credential {
        crate::model_router::Credential::OAuth2 {
            access_token,
            refresh_token,
            expires_at,
            token_url,
            client_id,
            scope,
            ..
        } => {
            println!("[providers.{}.auth_profile]", provider_id);
            if let Some(ref rt) = refresh_token {
                println!("refresh_token = \"{}\"", rt);
            }
            println!("access_token  = \"{}\"", access_token);
            println!("expires_at    = \"{}\"", expires_at.to_rfc3339());
            println!("token_url     = \"{}\"", token_url);
            println!("client_id     = \"{}\"", client_id);
            if let Some(ref s) = scope {
                println!("scope         = \"{}\"", s);
            }
        }
        _ => {
            println!("{:?}", credential);
        }
    }

    Ok(())
}
