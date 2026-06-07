//! Syscity Desktop App — Tauri backend
//!
//! Embeds the Syscity Gateway and serves it to the Tauri WebView.

use std::sync::Arc;

use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

/// Shared application state between Tauri commands.
pub struct AppState {
    /// Whether the embedded Gateway has finished startup.
    pub gateway_ready: bool,
    /// The actual port the Gateway bound to (auto-detected).
    pub gateway_port: u16,
}

/// Tauri command: returns the Gateway base URL for the frontend.
#[tauri::command]
fn get_api_url(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> String {
    let state = state.blocking_lock();
    format!("http://127.0.0.1:{}", state.gateway_port)
}

/// Entry point used by `src/main.rs`.
pub fn run() {
    // Initialize syscity global setup (panic handler, etc.)
    if let Err(e) = syscity::init() {
        eprintln!("Failed to initialize syscity: {}", e);
        std::process::exit(1);
    }

    // Set up a simple tracing subscriber so Gateway logs are visible.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let app_state = Arc::new(Mutex::new(AppState {
        gateway_ready: false,
        gateway_port: 18080,
    }));

    let app_state_for_setup = app_state.clone();

    tauri::Builder::default()
        .manage(app_state)
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![get_api_url])
        .setup(move |app| {
            let handle = app.handle().clone();
            let state = app_state_for_setup.clone();

            // Spawn the Syscity Gateway in a background task.
            tauri::async_runtime::spawn(async move {
                let port = match find_available_port("127.0.0.1", 18080, 100).await {
                    Some(p) => p,
                    None => {
                        eprintln!("No available port found in range 18080-18179");
                        return;
                    }
                };

                {
                    let mut s = state.lock().await;
                    s.gateway_port = port;
                }

                if let Err(e) = start_gateway(handle.clone(), port).await {
                    eprintln!("Gateway failed: {}", e);
                }
            });

            // Set up system tray and menu.
            if let Err(e) = setup_tray_and_menu(app) {
                eprintln!("Tray/menu setup failed: {}", e);
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Probe for an available TCP port starting at `start`.
async fn find_available_port(host: &str, start: u16, max_attempts: u16) -> Option<u16> {
    let end = start + max_attempts;
    for port in start..end {
        match tokio::net::TcpListener::bind(format!("{}:{}", host, port)).await {
            Ok(listener) => {
                // Port is available; drop the listener so Gateway can bind it.
                drop(listener);
                return Some(port);
            }
            Err(_) => continue,
        }
    }
    None
}

/// Start the embedded Syscity Gateway on the given port.
async fn start_gateway(
    handle: tauri::AppHandle,
    port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use syscity::gateway::{Gateway, GatewayConfig};

    // Ensure ~/.syscity directory exists.
    let syscity_dir = syscity::dirs::syscity_dir();
    let config_path = syscity_dir.join("syscity.toml");

    if !syscity_dir.exists() {
        tokio::fs::create_dir_all(&syscity_dir)
            .await
            .map_err(|e| format!("Failed to create syscity dir: {}", e))?;
    }

    // Load or create default Gateway config.
    let mut gateway_config = if config_path.exists() {
        match tokio::fs::read_to_string(&config_path).await {
            Ok(content) => match toml::from_str::<GatewayConfig>(&content) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("Failed to parse syscity.toml: {}, using defaults", e);
                    GatewayConfig::default()
                }
            },
            Err(e) => {
                eprintln!("Failed to read syscity.toml: {}, using defaults", e);
                GatewayConfig::default()
            }
        }
    } else {
        let default_config = r#"# Syscity Desktop Configuration
# Auto-generated on first start

[server]
host = "127.0.0.1"
port = 18080

[security]
enabled = true
auth_required = false
pairing_required = false
auth_mode = "none"
security_headers = true

[model]
model = "claude-3-sonnet-20240229"
model_provider = "anthropic"

[storage]
storage_type = "sqlite"

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
"#;
        tokio::fs::write(&config_path, default_config)
            .await
            .map_err(|e| format!("Failed to write default config: {}", e))?;
        GatewayConfig::default()
    };

    // Force localhost binding and use the auto-detected port.
    gateway_config.host = "127.0.0.1".to_string();
    gateway_config.port = port;

    // Configure LLM provider from environment (same logic as daemon.rs).
    if let (Ok(base_url), Ok(api_key)) =
        (std::env::var("SYSCITY_BASE_URL"), std::env::var("SYSCITY_API_KEY"))
    {
        let provider_config = syscity::model_router::ProviderConfig {
            provider_type: syscity::model_router::ProviderType::OpenAi,
            api_key,
            api_keys: Vec::new(),
            auth_profile: None,
            oauth: None,
            base_url: Some(base_url),
            timeout: std::time::Duration::from_secs(60),
            max_retries: 3,
            retry_delay_ms: 1000,
        };
        gateway_config
            .providers
            .insert("openai".to_string(), provider_config);
    } else if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        let provider_config = syscity::model_router::ProviderConfig {
            provider_type: syscity::model_router::ProviderType::Anthropic,
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
    } else if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        let provider_config = syscity::model_router::ProviderConfig {
            provider_type: syscity::model_router::ProviderType::OpenAi,
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
    }

    eprintln!(
        "Starting Syscity Gateway on http://{}:{}",
        gateway_config.host, gateway_config.port
    );

    let gateway = Gateway::new(gateway_config.clone(), Some(config_path))
        .await
        .map_err(|e| format!("Failed to create gateway: {}", e))?;

    // Show the main window now that the backend is ready.
    if let Some(window) = handle.get_webview_window("main") {
        window.show().unwrap();
    }

    // Notify the frontend that the backend is ready.
    handle
        .emit(
            "gateway-ready",
            format!("http://{}:{}", gateway_config.host, gateway_config.port),
        )
        .ok();

    // Blocks until the server shuts down.
    gateway
        .start()
        .await
        .map_err(|e| format!("Gateway error: {}", e))?;

    Ok(())
}

/// Set up the system tray icon and context menu.
fn setup_tray_and_menu(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::TrayIconBuilder;

    let handle = app.handle();

    let show_i = MenuItemBuilder::new("Show Window")
        .id("show")
        .build(handle)?;
    let quit_i = MenuItemBuilder::new("Quit").id("quit").build(handle)?;

    let tray_menu = MenuBuilder::new(handle)
        .item(&show_i)
        .separator()
        .item(&quit_i)
        .build()?;

    TrayIconBuilder::new()
        .icon(
            handle
                .default_window_icon()
                .ok_or("No default window icon")?
                .clone(),
        )
        .menu(&tray_menu)
        .on_menu_event(|app: &tauri::AppHandle, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(handle)?;

    Ok(())
}
