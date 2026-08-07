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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;

    async fn body(resp: axum::response::Response) -> (StatusCode, Vec<u8>) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec();
        (status, bytes)
    }

    #[tokio::test]
    async fn web_terminal_serves_html() {
        let (status, bytes) = body(web_terminal_html_handler().await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        let html = String::from_utf8_lossy(&bytes);
        assert!(
            html.contains("<html") || html.contains("Syscity Chat UI"),
            "serves html: {:.80}",
            html
        );
    }

    #[tokio::test]
    async fn favicon_served_with_png_mime() {
        let resp = favicon_handler().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "image/png");
        let (_, bytes) = body(resp).await;
        assert!(!bytes.is_empty());
    }

    #[tokio::test]
    async fn asset_missing_returns_404() {
        let (status, _) = body(
            asset_handler(Path("nope-xyz.bin".into()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn asset_found_returns_content() {
        let resp = asset_handler(Path("favicon.svg".into()))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let (_, bytes) = body(resp).await;
        assert!(!bytes.is_empty());
    }

    #[tokio::test]
    async fn register_sw_returns_javascript() {
        let resp = register_sw_handler().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "application/javascript");
        let (_, bytes) = body(resp).await;
        assert!(!bytes.is_empty());
    }

    #[tokio::test]
    async fn manifest_returns_json() {
        let resp = manifest_handler().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "application/manifest+json");
        let (_, bytes) = body(resp).await;
        assert!(!bytes.is_empty());
    }

    #[tokio::test]
    async fn syscity_png_served() {
        let resp = syscity_png_handler().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "image/png");
    }
}
