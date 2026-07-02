//! Plugin runtime types: shared state, per-plugin state, events, persistence.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::RwLock;

use super::super::manifest::{PluginManifest, PluginPermission};
use super::super::metrics::PluginMetricsRegistry;

/// Shared state accessible by all plugin instances.
///
/// Provides async-capable primitives (KV store, HTTP client, event bridge)
/// that synchronous WASM host functions delegate to via `block_on`.
#[cfg(feature = "plugins")]
pub struct PluginSharedState {
    /// Persistent per-plugin KV store
    pub kv_store: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    /// Global event channel (plugins emit, Syscity consumers subscribe)
    pub event_tx: Option<tokio::sync::mpsc::UnboundedSender<PluginEvent>>,
    /// Shared HTTP client
    pub http_client: reqwest::Client,
    /// Current session ID (set by Syscity when invoking plugins)
    pub session_id: Arc<RwLock<Option<String>>>,
    /// Arbitrary context map (set by Syscity)
    pub context: Arc<RwLock<HashMap<String, String>>>,
    /// Per-plugin metrics registry
    pub metrics: Arc<PluginMetricsRegistry>,
}

#[cfg(feature = "plugins")]
impl Default for PluginSharedState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "plugins")]
impl PluginSharedState {
    /// Create shared state without event channel
    pub fn new() -> Self {
        Self {
            kv_store: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
            http_client: Self::build_http_client(),
            session_id: Arc::new(RwLock::new(None)),
            context: Arc::new(RwLock::new(HashMap::new())),
            metrics: Arc::new(PluginMetricsRegistry::new()),
        }
    }

    /// Build an HTTP client with a 30-second timeout.
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    /// Create shared state with an event channel and metrics registry.
    pub fn with_events(
        event_tx: tokio::sync::mpsc::UnboundedSender<PluginEvent>,
        metrics: Arc<PluginMetricsRegistry>,
    ) -> Self {
        Self {
            kv_store: Arc::new(RwLock::new(HashMap::new())),
            event_tx: Some(event_tx),
            http_client: Self::build_http_client(),
            session_id: Arc::new(RwLock::new(None)),
            context: Arc::new(RwLock::new(HashMap::new())),
            metrics,
        }
    }

    /// Set the current session ID
    pub async fn set_session_id(&self, id: String) {
        *self.session_id.write().await = Some(id);
    }

    /// Get the current session ID
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Set a context value
    pub async fn set_context(&self, key: String, value: String) {
        self.context.write().await.insert(key, value);
    }

    /// Get a context value
    pub async fn get_context(&self, key: &str) -> Option<String> {
        self.context.read().await.get(key).cloned()
    }

    /// Get all context as JSON
    pub async fn get_all_context(&self) -> String {
        let ctx = self.context.read().await;
        serde_json::to_string(&*ctx).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Event emitted by plugins via `emit_event`
#[cfg(feature = "plugins")]
#[derive(Debug, Clone)]
pub struct PluginEvent {
    pub plugin_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

/// A loaded plugin instance
pub struct PluginInstance {
    /// Plugin manifest
    pub manifest: PluginManifest,
    /// Plugin directory path
    pub path: PathBuf,
    /// Whether the plugin is enabled
    pub enabled: bool,
    /// Plugin configuration
    pub config: serde_json::Value,
    /// WASM store (if loaded)
    #[cfg(feature = "plugins")]
    pub wasm_store: Option<wasmtime::Store<PluginState>>,
    #[cfg(feature = "plugins")]
    pub instance: Option<wasmtime::Instance>,
}

impl PluginInstance {
    /// Get plugin ID
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// Get plugin name
    pub fn name(&self) -> &str {
        &self.manifest.name
    }
}

/// Plugin state passed to WASM
#[cfg(feature = "plugins")]
pub struct PluginState {
    /// Plugin configuration
    pub config: serde_json::Value,
    /// Memory for plugin use
    pub memory: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    /// Shared state (KV store, HTTP, events, context)
    pub shared_state: Arc<PluginSharedState>,
    /// Plugin ID (for event emission, store scoping)
    pub plugin_id: String,
    /// Permissions granted to this plugin
    pub permissions: Vec<PluginPermission>,
    /// WASI context for sandboxed I/O (wrapped in StdMutex for Sync safety)
    pub wasi_ctx: StdMutex<wasmtime_wasi::p1::WasiP1Ctx>,
}

#[cfg(feature = "plugins")]
impl PluginState {
    pub fn new(
        config: serde_json::Value,
        shared_state: Arc<PluginSharedState>,
        plugin_id: String,
        permissions: Vec<PluginPermission>,
    ) -> Self {
        Self {
            config,
            memory: Arc::new(RwLock::new(HashMap::new())),
            shared_state,
            plugin_id,
            permissions,
            wasi_ctx: StdMutex::new(wasmtime_wasi::WasiCtxBuilder::new().build_p1()),
        }
    }

    pub fn new_with_memory(
        config: serde_json::Value,
        memory: HashMap<String, Vec<u8>>,
        shared_state: Arc<PluginSharedState>,
        plugin_id: String,
        permissions: Vec<PluginPermission>,
    ) -> Self {
        Self {
            config,
            memory: Arc::new(RwLock::new(memory)),
            shared_state,
            plugin_id,
            permissions,
            wasi_ctx: StdMutex::new(wasmtime_wasi::WasiCtxBuilder::new().build_p1()),
        }
    }
}

/// Current schema version for plugin persistent state.
pub(super) const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Migration record tracking applied schema migrations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct MigrationRecord {
    pub from_version: u32,
    pub to_version: u32,
    pub migrated_at: String,
}

/// Migration function type.
pub(super) type MigrationFn = fn(&mut PluginPersistentState) -> crate::Result<()>;

/// All registered migrations: (from, to, migrate_fn)
pub(super) fn get_migrations() -> Vec<(u32, u32, MigrationFn)> {
    vec![
        // v0 -> v1: bump schema version, add migration history
        (0, 1, migrate_v0_to_v1),
    ]
}

/// Migration from v0 (pre-schema-version format) to v1.
///
/// This just bumps the schema version and records the migration.
pub(super) fn migrate_v0_to_v1(state: &mut PluginPersistentState) -> crate::Result<()> {
    state.schema_version = 1;
    Ok(())
}

/// Serialisable snapshot of plugin state for disk persistence.
#[cfg(feature = "plugins")]
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct PluginPersistentState {
    /// Schema version for migration support. Defaults to 0 for backward compat.
    #[serde(default)]
    pub(super) schema_version: u32,
    pub(super) memory: HashMap<String, Vec<u8>>,
    pub(super) kv_store: HashMap<String, HashMap<String, String>>,
    /// History of applied migrations.
    #[serde(default)]
    pub(super) migration_history: Vec<MigrationRecord>,
}

impl Clone for PluginInstance {
    fn clone(&self) -> Self {
        // Note: WASM stores can't be cloned, so we skip them
        Self {
            manifest: self.manifest.clone(),
            path: self.path.clone(),
            enabled: self.enabled,
            config: self.config.clone(),
            #[cfg(feature = "plugins")]
            wasm_store: None,
            #[cfg(feature = "plugins")]
            instance: None,
        }
    }
}
