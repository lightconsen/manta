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
    let path = artifacts_dir.join(&filename);

    // Path traversal protection: canonicalize and verify it's under artifacts dir
    let canonical = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                "Document not found".to_string(),
            )
                .into_response();
        }
    };

    if !canonical.starts_with(&artifacts_dir) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Access denied".to_string(),
        )
            .into_response();
    }

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
