//! Artifact serving handler
//!
//! Serves files from `~/.syscity/artifacts/` for the document preview feature.
//! Documents written by the `write_document` tool are served here.

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::IntoResponse,
};

/// GET /api/v1/artifacts/{filename}
///
/// Reads a document from the artifacts directory and returns it with the
/// appropriate Content-Type (text/markdown for .md, text/html for .html).
pub async fn artifact_handler(
    Path(filename): Path<String>,
) -> impl IntoResponse {
    let artifacts_dir = crate::dirs::syscity_dir().join("artifacts");

    // Path traversal protection: reject slashes and ".." in the filename
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Access denied".to_string(),
        )
            .into_response();
    }

    let path = artifacts_dir.join(&filename);

    // Note: we intentionally skip canonicalize() here because on macOS it
    // fails on Unicode filenames due to NFC/NFD normalization differences
    // between the HTTP-decoded path and the filesystem-stored path.
    // The write_document tool already validates paths server-side.

    match tokio::fs::read_to_string(&path).await {
        Ok(content) => {
            let mime = if filename.ends_with(".html") {
                "text/html; charset=utf-8"
            } else {
                "text/markdown; charset=utf-8"
            };
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, mime)],
                content,
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Document not found".to_string(),
        )
            .into_response(),
    }
}
