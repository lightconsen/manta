#![allow(unused_imports)]

use axum::{
    extract::{Path, Query, State, WebSocketUpgrade},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, RwLock};
use tower_http::cors::CorsLayer;
use tracing::{debug, error, info, warn};

use crate::acp::AcpControlPlane;
use crate::agent::{Agent, AgentConfig};
use crate::canvas::{CanvasEvent, CanvasManager};
use crate::channels::{Channel, ChannelExtension, ChannelType};
use crate::config::hot_reload::{ConfigFileType, HotReloadManager};
use crate::inbound::*;
use crate::memory::vector::{
    ApiEmbeddingProvider, CachedEmbeddingProvider, EmbeddingConfig, LocalGgufEmbeddingProvider,
    MemoryVectorStore, VectorMemoryService,
};
use crate::model_router::ModelRouter;
use crate::plugins::PluginManager;
use crate::security::pairing::DmPolicy;
use crate::tools::approval::{ApprovalDecision, ApprovalFilter, ApprovalQueue};
use crate::tools::mcp::{McpManager, McpSettings, McpToolWrapper};
use crate::tools::ToolRegistry;
use crate::gateway::GatewayState;
use crate::gateway::*;

/// HTML handler for the web chat UI
///
/// Serves the built React app from embedded assets (or filesystem fallback).
pub async fn web_terminal_html_handler() -> Html<String> {
    let html = match crate::embed::WebAssets::get("index.html") {
        Some(file) => String::from_utf8_lossy(file.data.as_ref()).to_string(),
        None => tokio::fs::read_to_string("dist/index.html")
            .await
            .unwrap_or_else(|_| {
                "<h1>Syscity Chat UI</h1><p>Build not found. Run: cd web and pnpm build</p>"
                    .to_string()
            }),
    };
    Html(html.replace("{VERSION}", crate::VERSION))
}

/// Favicon handler — serves the syscity ray SVG favicon
pub async fn favicon_handler() -> impl IntoResponse {
    let svg = match crate::embed::WebAssets::get("favicon.svg") {
        Some(file) => String::from_utf8_lossy(file.data.as_ref()).to_string(),
        None => tokio::fs::read_to_string("dist/favicon.svg")
            .await
            .unwrap_or_else(|_| {
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 80"><path d="M50 8C50 8 38 0 28 8C18 16 8 24 2 36C-2 44 2 52 10 48C18 44 22 40 26 36C30 32 34 28 38 30C42 32 44 38 44 46C44 54 42 64 40 72C38 76 42 78 44 74C46 66 48 56 50 50C52 56 54 66 56 74C58 78 62 76 60 72C58 64 56 54 56 46C56 38 58 32 62 30C66 28 70 32 74 36C78 40 82 44 90 48C98 52 102 44 98 36C92 24 82 16 72 8C62 0 50 8 50 8Z" fill="#10b981"/><circle cx="38" cy="18" r="2" fill="white"/><circle cx="62" cy="18" r="2" fill="white"/></svg>"##.to_string()
            }),
    };
    ([(header::CONTENT_TYPE, "image/svg+xml")], svg)
}

/// Asset handler — serves JS/CSS/fonts from embedded assets (or filesystem fallback).
pub async fn asset_handler(Path(path): Path<String>) -> impl IntoResponse {
    // Try embedded assets first (handles both direct keys and "assets/" prefix).
    if let Some((data, mime)) = crate::embed::get_asset(&path) {
        return ([(header::CONTENT_TYPE, mime)], data).into_response();
    }

    // Fallback to filesystem for development
    let fs_paths = [
        format!("dist/{}", path),
        format!("dist/assets/{}", path),
    ];
    for fs_path in &fs_paths {
        if let Ok(data) = tokio::fs::read(fs_path).await {
            let mime = crate::embed::guess_mime(&path);
            return ([(header::CONTENT_TYPE, mime)], data).into_response();
        }
    }

    StatusCode::NOT_FOUND.into_response()
}

/// Logo handler for /syscity.png — static route with no path params.
pub async fn syscity_png_handler() -> impl IntoResponse {
    let path = "syscity.png";
    if let Some((data, mime)) = crate::embed::get_asset(path) {
        return ([(header::CONTENT_TYPE, mime)], data).into_response();
    }
    if let Ok(data) = tokio::fs::read(format!("dist/{}", path)).await {
        let mime = crate::embed::guess_mime(path);
        return ([(header::CONTENT_TYPE, mime)], data).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// Admin redirect handler — admin UI moved to CLI
pub async fn admin_redirect_handler() -> Html<&'static str> {
    Html("<h1>Admin UI Moved</h1><p>Administration is now available via CLI: <code>syscity admin</code></p>")
}

