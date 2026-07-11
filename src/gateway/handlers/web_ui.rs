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
            "<h1>Syscity Chat UI</h1><p>Build not found. Run: cd web and pnpm build</p>"
                .to_string()
        }
    };
    Html(html.replace("{VERSION}", crate::VERSION))
}

/// Favicon handler — serves the syscity ray SVG favicon
pub async fn favicon_handler() -> impl IntoResponse {
    let svg = crate::embed::get_asset_string("favicon.svg").unwrap_or_else(|| {
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 80"><path d="M50 8C50 8 38 0 28 8C18 16 8 24 2 36C-2 44 2 52 10 48C18 44 22 40 26 36C30 32 34 28 38 30C42 32 44 38 44 46C44 54 42 64 40 72C38 76 42 78 44 74C46 66 48 56 50 50C52 56 54 66 56 74C58 78 62 76 60 72C58 64 56 54 56 46C56 38 58 32 62 30C66 28 70 32 74 36C78 40 82 44 90 48C98 52 102 44 98 36C92 24 82 16 72 8C62 0 50 8 50 8Z" fill="#10b981"/><circle cx="38" cy="18" r="2" fill="white"/><circle cx="62" cy="18" r="2" fill="white"/></svg>"##.to_string()
    });
    ([(header::CONTENT_TYPE, "image/svg+xml")], svg)
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
