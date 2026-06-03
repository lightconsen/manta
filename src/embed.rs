//! Embedded web frontend assets
//!
//! Uses rust-embed to compile the built React app (`web/dist/`) into the binary.
//! This allows distributing Syscity as a single executable without requiring
//! the `web/dist/` directory at runtime.

use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/dist/"]
pub struct WebAssets;

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
