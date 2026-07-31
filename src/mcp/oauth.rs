//! OAuth 2.0 token management — types, manager handle, background actor,
//! token persistence, and the PKCE authorization callback server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{info, warn};

use crate::mcp::{McpEvent, McpManager, McpServerConfig};

// ─────────────────────────────────────────────
// Token data
// ─────────────────────────────────────────────

/// OAuth 2.0 token data persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    /// Token endpoint URL used for refreshing.
    pub token_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// OAuth client ID used for token refresh.
    pub client_id: String,
}

// ─────────────────────────────────────────────
// Actor types
// ─────────────────────────────────────────────

/// Internal state for a pending OAuth flow (inside the actor only).
struct PendingFlowState {
    cancel_tx: oneshot::Sender<()>,
}

/// A cached token plus the disk file's last-modified time. The disk file is
/// authoritative: if its mtime differs from the cached value the cache entry
/// is stale (externally modified or deleted) and must be reloaded.
struct CachedToken {
    tokens: OAuthTokens,
    mtime: Option<SystemTime>,
}

/// Commands sent to the `OAuthManagerActor` via its mpsc channel.
pub(crate) enum OAuthCommand {
    StartFlow {
        server_id: String,
        config: McpServerConfig,
        resp_tx: oneshot::Sender<crate::Result<String>>,
    },
    CancelFlow {
        server_id: String,
    },
    GetToken {
        server_id: String,
        resp_tx: oneshot::Sender<Option<OAuthTokens>>,
    },
    HasToken {
        server_id: String,
        resp_tx: oneshot::Sender<bool>,
    },
    CallbackComplete {
        server_id: String,
        result: crate::Result<OAuthTokens>,
    },
    /// Drop the cached token and delete the persisted token file (e.g. when a
    /// server is removed).
    ClearToken {
        server_id: String,
    },
    /// Request the actor to shut down (currently unused, available for
    /// graceful teardown).
    #[allow(dead_code)]
    Shutdown,
}

// ─────────────────────────────────────────────
// OAuthManager handle
// ─────────────────────────────────────────────

/// Cloneable handle to the `OAuthManagerActor` background task.
#[derive(Clone)]
pub struct OAuthManager {
    pub(crate) cmd_tx: mpsc::UnboundedSender<OAuthCommand>,
}

impl OAuthManager {
    /// Create a new `OAuthManager` handle and the receiver half for the actor.
    pub(crate) fn new() -> (Self, mpsc::UnboundedReceiver<OAuthCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        (Self { cmd_tx }, cmd_rx)
    }

    /// Start an OAuth 2.1 + PKCE flow. Returns the authorization URL.
    pub async fn start_flow(
        &self,
        server_id: String,
        config: McpServerConfig,
    ) -> crate::Result<String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(OAuthCommand::StartFlow {
                server_id,
                config,
                resp_tx,
            });
        resp_rx
            .await
            .map_err(|_| crate::error::SyscityError::Internal("OAuth manager dropped".to_string()))?
    }

    /// Cancel a pending OAuth flow.
    pub async fn cancel_flow(&self, server_id: String) {
        let _ = self
            .cmd_tx
            .send(OAuthCommand::CancelFlow { server_id });
    }

    /// Get cached OAuth tokens for a server.
    pub async fn get_token(&self, server_id: String) -> Option<OAuthTokens> {
        let (resp_tx, resp_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(OAuthCommand::GetToken {
                server_id,
                resp_tx,
            });
        resp_rx.await.ok().flatten()
    }

    /// Check if valid (non-expired) tokens exist for a server.
    pub async fn has_token(&self, server_id: String) -> bool {
        let (resp_tx, resp_rx) = oneshot::channel();
        let _ = self
            .cmd_tx
            .send(OAuthCommand::HasToken {
                server_id,
                resp_tx,
            });
        resp_rx.await.unwrap_or(false)
    }

    /// Clear the cached token and delete the persisted token file.
    pub async fn clear_token(&self, server_id: String) {
        let _ = self
            .cmd_tx
            .send(OAuthCommand::ClearToken { server_id });
    }
}

// ─────────────────────────────────────────────
// OAuthManagerActor — background task
// ─────────────────────────────────────────────

/// Background actor that owns the token cache and manages OAuth flows.
pub(crate) struct OAuthManagerActor {
    cmd_rx: mpsc::UnboundedReceiver<OAuthCommand>,
    cmd_tx: mpsc::UnboundedSender<OAuthCommand>,
    event_tx: Arc<RwLock<Option<mpsc::UnboundedSender<McpEvent>>>>,
    token_cache: HashMap<String, CachedToken>,
    pending_flows: HashMap<String, PendingFlowState>,
}

impl OAuthManagerActor {
    pub(crate) fn new(
        cmd_rx: mpsc::UnboundedReceiver<OAuthCommand>,
        cmd_tx: mpsc::UnboundedSender<OAuthCommand>,
        event_tx: Arc<RwLock<Option<mpsc::UnboundedSender<McpEvent>>>>,
    ) -> Self {
        Self {
            cmd_rx,
            cmd_tx,
            event_tx,
            token_cache: HashMap::new(),
            pending_flows: HashMap::new(),
        }
    }

    /// Spawn the actor as a background task.
    pub(crate) fn spawn(self) {
        tokio::spawn(self.run());
    }

    /// Main run loop: process commands + periodic token refresh.
    async fn run(mut self) {
        self.preload_cache().await;

        let mut refresh_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        None => break,
                        Some(OAuthCommand::StartFlow { server_id, config, resp_tx }) => {
                            let _ = resp_tx.send(self.handle_start_flow(&server_id, &config).await);
                        }
                        Some(OAuthCommand::CancelFlow { server_id }) => {
                            self.handle_cancel_flow(&server_id).await;
                        }
                        Some(OAuthCommand::GetToken { server_id, resp_tx }) => {
                            let _ = resp_tx.send(self.handle_get_token(&server_id).await);
                        }
                        Some(OAuthCommand::HasToken { server_id, resp_tx }) => {
                            let _ = resp_tx.send(self.handle_has_token(&server_id).await);
                        }
                        Some(OAuthCommand::CallbackComplete { server_id, result }) => {
                            self.handle_callback_complete(&server_id, result).await;
                        }
                        Some(OAuthCommand::ClearToken { server_id }) => {
                            self.handle_clear_token(&server_id).await;
                        }
                        Some(OAuthCommand::Shutdown) => break,
                    }
                }
                _ = refresh_interval.tick() => {
                    self.refresh_expiring_tokens().await;
                }
            }
        }
    }

    /// Preload token cache by reading all token files from disk.
    async fn preload_cache(&mut self) {
        let dir = mcp_tokens_dir();
        if !dir.exists() {
            return;
        }
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => return,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(String::from) {
                if let Ok(data) = tokio::fs::read_to_string(&path).await {
                    if let Ok(tokens) = serde_json::from_str::<OAuthTokens>(&data) {
                        let mtime = disk_mtime(&path).await;
                        self.token_cache
                            .insert(id, CachedToken { tokens, mtime });
                    }
                }
            }
        }
    }

    async fn emit_event(&self, event: McpEvent) {
        if let Some(tx) = self.event_tx.read().await.as_ref() {
            let _ = tx.send(event);
        }
    }

    async fn handle_start_flow(
        &mut self,
        server_id: &str,
        config: &McpServerConfig,
    ) -> crate::Result<String> {
        let url = config.url.as_deref().ok_or_else(|| {
            crate::error::SyscityError::Internal("Remote MCP server has no URL".to_string())
        })?;

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| {
                crate::error::SyscityError::Internal(format!(
                    "Failed to build HTTP client: {e}"
                ))
            })?;

        // 1. Discover OAuth endpoints: explicit config → RFC 8414 well-known →
        //    RFC 9728 protected-resource metadata → known providers (GitHub).
        let (authorization_endpoint, token_endpoint) =
            discover_oauth_endpoints(&http_client, url, config)
                .await
                .map_err(|e| {
                    crate::error::SyscityError::Internal(format!(
                        "OAuth discovery failed for '{server_id}': {e}"
                    ))
                })?;

        // 2. Generate PKCE challenge
        let code_verifier = McpManager::generate_code_verifier();
        let code_challenge = McpManager::generate_code_challenge(&code_verifier);
        let state = McpManager::generate_random_state();

        let client_id = config
            .client_id
            .clone()
            .unwrap_or_else(|| "syscity".to_string());

        let scopes = config.scopes.clone().unwrap_or_default();

        // 3. Bind local callback listener
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to bind callback port: {e}"))
        })?;
        let callback_port = listener.local_addr().map_err(|e| {
            crate::error::SyscityError::Internal(format!("Failed to get local addr: {e}"))
        })?.port();

        let redirect_uri = format!("http://127.0.0.1:{callback_port}/callback");

        // 4. Build authorization URL
        let mut auth_url = format!(
            "{authorization_endpoint}?response_type=code&client_id={}&redirect_uri={}&code_challenge={code_challenge}&code_challenge_method=S256&state={state}",
            urlencoding(&client_id),
            urlencoding(&redirect_uri),
        );
        if !scopes.is_empty() {
            auth_url.push_str(&format!("&scope={}", urlencoding(&scopes)));
        }

        // 5. Create cancel channel
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        // 6. Store pending flow state
        self.pending_flows.insert(
            server_id.to_string(),
            PendingFlowState { cancel_tx },
        );

        // 7. Spawn callback server task (sends CallbackComplete back to actor)
        let sv_id = server_id.to_string();
        let cmd_tx = self.cmd_tx.clone();
        tokio::spawn(async move {
            let result = run_callback_server(
                listener,
                &token_endpoint,
                &code_verifier,
                &state,
                &client_id,
                &redirect_uri,
                cancel_rx,
            )
            .await;

            let _ = cmd_tx.send(OAuthCommand::CallbackComplete {
                server_id: sv_id,
                result,
            });
        });

        // Notify clients that a flow has started so the UI can surface the
        // authorization URL even when the connect call came from elsewhere
        // (e.g. CLI or REST).
        self.emit_event(McpEvent::AuthRequired {
            server_id: server_id.to_string(),
            auth_url: auth_url.clone(),
        })
        .await;

        Ok(auth_url)
    }

    async fn handle_cancel_flow(&mut self, server_id: &str) {
        if let Some(flow) = self.pending_flows.remove(server_id) {
            let _ = flow.cancel_tx.send(());
            self.emit_event(McpEvent::AuthFailed {
                server_id: server_id.to_string(),
                reason: "cancelled_by_user".to_string(),
            })
            .await;
        }
    }

    async fn handle_get_token(&mut self, server_id: &str) -> Option<OAuthTokens> {
        let path = token_path_for(server_id);
        let disk_mtime = disk_mtime(&path).await;

        // Cache hit only if the disk file's mtime matches — otherwise the
        // cache is stale (externally edited/deleted token) and must be
        // reloaded from disk.
        if let Some(cached) = self.token_cache.get(server_id) {
            if cached.mtime == disk_mtime {
                return Some(cached.tokens.clone());
            }
            self.token_cache.remove(server_id);
        }

        let data = tokio::fs::read_to_string(&path).await.ok()?;
        let tokens: OAuthTokens = serde_json::from_str(&data).ok()?;
        self.token_cache.insert(
            server_id.to_string(),
            CachedToken {
                tokens: tokens.clone(),
                mtime: disk_mtime,
            },
        );
        Some(tokens)
    }

    async fn handle_has_token(&mut self, server_id: &str) -> bool {
        let Some(tokens) = self.handle_get_token(server_id).await else {
            return false;
        };

        if let Some(expires_at) = tokens.expires_at {
            let now = chrono::Utc::now().timestamp();
            if now >= expires_at - 60 {
                return false;
            }
        }
        true
    }

    async fn handle_clear_token(&mut self, server_id: &str) {
        self.token_cache.remove(server_id);
        let path = token_path_for(server_id);
        if let Err(e) = tokio::fs::remove_file(&path).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!("Failed to delete token file for '{server_id}': {e}");
            }
        }
        info!("Cleared stored OAuth token for '{server_id}'");
    }

    async fn handle_callback_complete(
        &mut self,
        server_id: &str,
        result: crate::Result<OAuthTokens>,
    ) {
        // Clean up pending flow
        self.pending_flows.remove(server_id);

        match result {
            Ok(tokens) => {
                // Persist to disk first, then cache with the file's mtime so
                // the cache matches the authoritative on-disk state.
                let tokens_dir = mcp_tokens_dir();
                let _ = tokio::fs::create_dir_all(&tokens_dir).await;
                let token_path = tokens_dir.join(format!("{server_id}.json"));
                if let Ok(json) = serde_json::to_string(&tokens) {
                    let _ = tokio::fs::write(&token_path, &json).await;
                }
                let mtime = disk_mtime(&token_path).await;
                self.token_cache.insert(
                    server_id.to_string(),
                    CachedToken {
                        tokens: tokens.clone(),
                        mtime,
                    },
                );

                self.emit_event(McpEvent::AuthComplete {
                    server_id: server_id.to_string(),
                })
                .await;
            }
            Err(e) => {
                warn!("OAuth flow failed for '{server_id}': {e}");
                self.emit_event(McpEvent::AuthFailed {
                    server_id: server_id.to_string(),
                    reason: e.to_string(),
                })
                .await;
            }
        }
    }

    /// Iterate the token cache and refresh any tokens expiring within 5 minutes.
    async fn refresh_expiring_tokens(&mut self) {
        let now = chrono::Utc::now().timestamp();
        let refresh_window = now + 300; // 5 minutes from now

        // Collect expiring server IDs + their token data to avoid borrow
        // conflicts.
        let expiring: Vec<(String, OAuthTokens)> = self
            .token_cache
            .iter()
            .filter(|(_, c)| {
                c.tokens
                    .expires_at
                    .is_some_and(|exp| exp <= refresh_window)
                    && c.tokens.refresh_token.is_some()
            })
            .map(|(id, c)| (id.clone(), c.tokens.clone()))
            .collect();

        for (server_id, tokens) in &expiring {
            let http_client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
            {
                Ok(c) => c,
                Err(_) => continue,
            };

            let body = format!(
                "grant_type=refresh_token&refresh_token={}&client_id={}",
                tokens.refresh_token.as_deref().unwrap_or(""),
                tokens.client_id
            );

            match http_client
                .post(&tokens.token_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .header("Accept", "application/json")
                .body(body)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(data) => {
                            let new_access = data["access_token"]
                                .as_str()
                                .map(String::from)
                                .unwrap_or_else(|| tokens.access_token.clone());
                            let new_refresh = data["refresh_token"]
                                .as_str()
                                .map(String::from)
                                .or_else(|| tokens.refresh_token.clone());
                            let new_expires = data["expires_in"]
                                .as_i64()
                                .map(|secs| chrono::Utc::now().timestamp() + secs);

                            let updated = OAuthTokens {
                                access_token: new_access,
                                token_url: tokens.token_url.clone(),
                                refresh_token: new_refresh,
                                expires_at: new_expires,
                                client_id: tokens.client_id.clone(),
                            };

                            // Persist to disk, then cache with the file's mtime
                            let tokens_dir = mcp_tokens_dir();
                            let token_path =
                                tokens_dir.join(format!("{server_id}.json"));
                            if let Ok(json) = serde_json::to_string(&updated) {
                                let _ = tokio::fs::write(&token_path, &json).await;
                            }
                            let mtime = disk_mtime(&token_path).await;
                            self.token_cache.insert(
                                server_id.clone(),
                                CachedToken {
                                    tokens: updated,
                                    mtime,
                                },
                            );

                            info!("OAuth token refreshed for '{server_id}'");

                            self.emit_event(McpEvent::TokenRefreshed {
                                server_id: server_id.clone(),
                            })
                            .await;
                        }
                        Err(e) => {
                            warn!(
                                "Failed to parse refresh token response for '{server_id}': {e}"
                            );
                        }
                    }
                }
                Ok(resp) => {
                    warn!(
                        "Token refresh failed for '{server_id}': HTTP {}",
                        resp.status()
                    );
                }
                Err(e) => {
                    warn!("Token refresh request failed for '{server_id}': {e}");
                }
            }
        }
    }
}

// ─────────────────────────────────────────────
// Token persistence
// ─────────────────────────────────────────────

/// Directory for MCP OAuth token storage (~/.syscity/mcp_tokens).
pub fn mcp_tokens_dir() -> PathBuf {
    crate::dirs::syscity_dir().join("mcp_tokens")
}

/// Path to the token file for a specific server.
pub fn token_path_for(server_id: &str) -> PathBuf {
    mcp_tokens_dir().join(format!("{}.json", server_id))
}

/// Last-modified time of a file, or `None` if it does not exist. Falls back
/// to UNIX_EPOCH on filesystems without modification-time support so the
/// cache still matches disk.
async fn disk_mtime(path: &Path) -> Option<SystemTime> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some(meta.modified().unwrap_or(std::time::UNIX_EPOCH))
}

/// Minimal percent-encoding for OAuth URL parameters.
fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{byte:02X}")),
        }
    }
    result
}

// ─────────────────────────────────────────────
// OAuth endpoint discovery
// ─────────────────────────────────────────────

/// An `(authorization_endpoint, token_endpoint)` pair.
type DiscoveredEndpoints = (String, String);

/// Discover OAuth endpoints for a remote MCP server.
///
/// Strategy (in order of preference):
/// 1. Explicit `auth_url` / `token_url` from the server config.
/// 2. RFC 8414 well-known metadata at the server origin
///    (`/.well-known/oauth-authorization-server`).
/// 3. RFC 9728 protected-resource metadata: the server advertises a
///    `resource_metadata` URL via the `WWW-Authenticate` challenge; that
///    document lists `authorization_servers`, each probed for RFC 8414 / OIDC
///    metadata.
/// 4. A registry of known providers that publish no discovery document at all
///    (e.g. GitHub's OAuth endpoints).
async fn discover_oauth_endpoints(
    http: &reqwest::Client,
    server_url: &str,
    config: &McpServerConfig,
) -> Result<DiscoveredEndpoints, String> {
    // 1. Explicit config wins.
    if let (Some(auth), Some(token)) = (config.auth_url.as_deref(), config.token_url.as_deref()) {
        return Ok((auth.to_string(), token.to_string()));
    }

    let origin = McpManager::origin_from_url(server_url);

    // 2. RFC 8414 well-known document at the server origin.
    let well_known = format!("{origin}/.well-known/oauth-authorization-server");
    if let Some(endpoints) = fetch_metadata_endpoints(http, &well_known).await {
        return Ok(endpoints);
    }

    // 3. RFC 9728 protected-resource metadata.
    let mut auth_servers: Vec<String> = Vec::new();
    if let Some(resource_meta_url) = fetch_resource_metadata_url(http, server_url).await {
        auth_servers = fetch_authorization_servers(http, &resource_meta_url).await;
        for issuer in &auth_servers {
            let rfc8414 = format!("{issuer}/.well-known/oauth-authorization-server");
            if let Some(endpoints) = fetch_metadata_endpoints(http, &rfc8414).await {
                return Ok(endpoints);
            }
            let oidc = format!("{issuer}/.well-known/openid-configuration");
            if let Some(endpoints) = fetch_metadata_endpoints(http, &oidc).await {
                return Ok(endpoints);
            }
        }
    }

    // 4. Known-provider registry, matched against the advertised authorization
    //    servers first, then the server origin.
    let issuers = auth_servers
        .iter()
        .map(|s| s.as_str())
        .chain(std::iter::once(origin.as_str()));
    for issuer in issuers {
        if let Some(endpoints) = known_provider_endpoints(issuer) {
            return Ok(endpoints);
        }
    }

    Err(format!(
        "the server exposes no OAuth metadata ({well_known} is unavailable and no \
         supported resource-metadata discovery was found); set auth_url and token_url \
         in the server config to use it directly"
    ))
}

/// Fetch an RFC 8414 / OIDC discovery document and extract the OAuth endpoints.
async fn fetch_metadata_endpoints(
    http: &reqwest::Client,
    metadata_url: &str,
) -> Option<DiscoveredEndpoints> {
    let resp = http.get(metadata_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let doc: serde_json::Value = resp.json().await.ok()?;
    let authorization_endpoint = doc.get("authorization_endpoint")?.as_str()?;
    let token_endpoint = doc.get("token_endpoint")?.as_str()?;
    Some((authorization_endpoint.to_string(), token_endpoint.to_string()))
}

/// Send an unauthenticated request to the MCP endpoint and return the
/// `resource_metadata` URL advertised in the `WWW-Authenticate` challenge
/// (RFC 9728). Probes with a POST `initialize` first, then a plain GET.
async fn fetch_resource_metadata_url(
    http: &reqwest::Client,
    server_url: &str,
) -> Option<String> {
    let request = http
        .post(server_url)
        .header("Accept", "application/json, text/event-stream")
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "syscity", "version": env!("CARGO_PKG_VERSION") },
            }
        }));
    let resp = request.send().await.ok()?;
    if let Some(meta) = www_authenticate_resource_metadata(&resp) {
        return Some(meta);
    }

    let resp = http.get(server_url).send().await.ok()?;
    www_authenticate_resource_metadata(&resp)
}

/// Extract the `resource_metadata` parameter from a response's
/// `WWW-Authenticate` header if present.
fn www_authenticate_resource_metadata(resp: &reqwest::Response) -> Option<String> {
    let header = resp.headers().get("www-authenticate")?.to_str().ok()?;
    extract_resource_metadata(header)
}

/// Extract the `resource_metadata` value from a `WWW-Authenticate` header,
/// e.g. `Bearer ..., resource_metadata="https://example.com/metadata"`.
fn extract_resource_metadata(www_authenticate: &str) -> Option<String> {
    let needle = "resource_metadata=";
    let idx = www_authenticate.find(needle)?;
    let after = &www_authenticate[idx + needle.len()..];
    let trimmed = after.trim_start();
    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    } else {
        let end = after.find(',').unwrap_or(after.len());
        Some(after[..end].trim().to_string())
    }
}

/// Fetch an RFC 9728 protected-resource metadata document and return the
/// listed authorization server issuers.
async fn fetch_authorization_servers(
    http: &reqwest::Client,
    resource_meta_url: &str,
) -> Vec<String> {
    let Ok(resp) = http.get(resource_meta_url).send().await else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(doc) = resp.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    doc.get("authorization_servers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Hardcoded OAuth endpoints for providers that publish no discovery document.
/// Keyed by hostname (scheme and path are ignored).
fn known_provider_endpoints(issuer: &str) -> Option<DiscoveredEndpoints> {
    let host = issuer
        .strip_prefix("https://")
        .or_else(|| issuer.strip_prefix("http://"))
        .unwrap_or(issuer)
        .split('/')
        .next()
        .unwrap_or(issuer);
    match host {
        "github.com" | "api.github.com" => Some((
            "https://github.com/login/oauth/authorize".to_string(),
            "https://github.com/login/oauth/access_token".to_string(),
        )),
        _ => None,
    }
}

// ─────────────────────────────────────────────
// Callback server
// ─────────────────────────────────────────────

/// Run a mini HTTP server handling the OAuth redirect callback,
/// exchanging the authorization code for tokens.
async fn run_callback_server(
    listener: TcpListener,
    token_url: &str,
    code_verifier: &str,
    expected_state: &str,
    client_id: &str,
    redirect_uri: &str,
    cancel_rx: oneshot::Receiver<()>,
) -> crate::Result<OAuthTokens> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let accept = Box::pin(listener.accept());
    let cancel = Box::pin(cancel_rx);

    let (stream, _) = tokio::select! {
        result = accept => result.map_err(|e| {
            crate::error::SyscityError::Internal(format!("Callback server accept failed: {e}"))
        })?,
        _ = cancel => {
            return Err(crate::error::SyscityError::Internal(
                "OAuth flow cancelled".to_string(),
            ));
        }
    };

    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = tokio::io::BufReader::new(reader).lines();

    let request_line = lines
        .next_line()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    let path = request_line.split_whitespace().nth(1).unwrap_or("/");

    // Drain remaining request headers
    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            break;
        }
    }

    // Parse query parameters
    let params: HashMap<String, String> = path
        .split('?')
        .nth(1)
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next()?.to_string();
            let value = parts.next().unwrap_or("").to_string();
            Some((key, value))
        })
        .collect();

    let code = params.get("code").ok_or_else(|| {
        crate::error::SyscityError::Internal("Missing code in OAuth callback".to_string())
    })?;

    let state = params.get("state").ok_or_else(|| {
        crate::error::SyscityError::Internal(
            "Missing state in OAuth callback".to_string(),
        )
    })?;

    if state != expected_state {
        let body = "Invalid state parameter. Authorization failed.";
        let _ = writer
            .write_all(
                format!(
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nContent-Type: text/plain\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await;
        return Err(crate::error::SyscityError::Internal(
            "State mismatch in OAuth callback".to_string(),
        ));
    }

    // Exchange code for tokens
    let http_client = reqwest::Client::new();
    let token_body = format!(
        "grant_type=authorization_code&code={code}&redirect_uri={redirect_uri}&client_id={client_id}&code_verifier={code_verifier}"
    );

    let token_response = http_client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(token_body)
        .send()
        .await
        .map_err(|e| {
            crate::error::SyscityError::Internal(format!("Token exchange failed: {e}"))
        })?;

    if !token_response.status().is_success() {
        let status = token_response.status();
        let error_text = token_response.text().await.unwrap_or_default();
        return Err(crate::error::SyscityError::Internal(format!(
            "Token exchange failed: HTTP {status} - {error_text}"
        )));
    }

    let token_data: serde_json::Value = token_response.json().await.map_err(|e| {
        crate::error::SyscityError::Internal(format!("Failed to parse token response: {e}"))
    })?;

    let access_token = token_data["access_token"]
        .as_str()
        .ok_or_else(|| {
            crate::error::SyscityError::Internal(
                "Missing access_token in token response".to_string(),
            )
        })?
        .to_string();

    let refresh_token = token_data["refresh_token"].as_str().map(String::from);
    let expires_at = token_data["expires_in"]
        .as_i64()
        .map(|secs| chrono::Utc::now().timestamp() + secs);

    let body = "<html><body><h1>Authorization complete!</h1><p>You may close this window and return to Syscity.</p></body></html>";
    let _ = writer
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await;

    Ok(OAuthTokens {
        access_token,
        token_url: token_url.to_string(),
        refresh_token,
        expires_at,
        client_id: client_id.to_string(),
    })
}
