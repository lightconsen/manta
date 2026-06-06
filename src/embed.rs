//! Embedded web frontend assets
//!
//! Uses rust-embed to compile the built React app (`dist/`) into the binary.
//! This allows distributing Syscity as a single executable without requiring
//! the `dist/` directory at runtime.

use rust_embed::Embed;

#[derive(Embed)]
#[folder = "dist/"]
pub struct WebAssets;

/// Look up an asset by its request path.
///
/// Vite places hashed JS/CSS under `dist/assets/`, so the embedded key
/// is either the direct path or `"assets/{path}"`.  This helper tries both
/// and returns `(bytes, mime_type)` on success.
pub fn get_asset(path: &str) -> Option<(Vec<u8>, &'static str)> {
    let keys = [path.to_string(), format!("assets/{}", path)];
    for key in &keys {
        if let Some(file) = WebAssets::get(key) {
            let mime = guess_mime(key);
            return Some((file.data.to_vec(), mime));
        }
    }
    None
}

/// Guess MIME type from file extension for embedded assets.
pub fn guess_mime(path: &str) -> &'static str {
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else if path.ends_with(".html") {
        "text/html"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else {
        "application/octet-stream"
    }
}
