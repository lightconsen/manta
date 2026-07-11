use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::{Html, IntoResponse},
};

/// HTML handler for the web chat UI
///
/// Serves the built React app from embedded assets (or filesystem fallback).
pub async fn web_terminal_html_handler() -> Html<String> {
    let html = match crate::embed::get_asset_string("index.html") {
        Some(html) => html,
        None => {
            "<h1>Syscity Chat UI</h1><p>Build not found. Run: cd web and pnpm build</p>".to_string()
        }
    };
    Html(html.replace("{VERSION}", crate::VERSION))
}

/// Favicon handler — serves the syscity PNG favicon
pub async fn favicon_handler() -> impl IntoResponse {
    if let Some((data, mime)) = crate::embed::get_asset("syscity.png") {
        return ([(header::CONTENT_TYPE, mime)], data).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// Asset handler — serves JS/CSS/fonts from embedded assets (or filesystem
/// fallback).
pub async fn asset_handler(Path(path): Path<String>) -> impl IntoResponse {
    // Try embedded assets first (handles both direct keys and "assets/" prefix).
    if let Some((data, mime)) = crate::embed::get_asset(&path) {
        return ([(header::CONTENT_TYPE, mime)], data).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}

/// Service worker handler — serves the Vite PWA service worker registration
/// script.
pub async fn register_sw_handler() -> impl IntoResponse {
    let js = crate::embed::get_asset_string("registerSW.js").unwrap_or_else(|| {
        "if('serviceWorker' in \
         navigator){window.addEventListener('load',()=>{navigator.serviceWorker.register('.\
         /sw.js',{scope:'./'})})}"
            .to_string()
    });
    ([(header::CONTENT_TYPE, "application/javascript")], js)
}

/// Web app manifest handler — serves the Vite PWA manifest.
pub async fn manifest_handler() -> impl IntoResponse {
    let manifest = crate::embed::get_asset_string("manifest.webmanifest").unwrap_or_default();
    ([(header::CONTENT_TYPE, "application/manifest+json")], manifest)
}

/// Logo handler for /syscity.png — static route with no path params.
pub async fn syscity_png_handler() -> impl IntoResponse {
    let path = "syscity.png";
    if let Some((data, mime)) = crate::embed::get_asset(path) {
        return ([(header::CONTENT_TYPE, mime)], data).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}
