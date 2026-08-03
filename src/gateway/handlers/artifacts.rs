//! Artifact serving handler
//!
//! Serves files from `~/.syscity/artifacts/` for the document preview feature.
//! Documents written by the `write_report` tool are served here.
//!
//! The route is a wildcard so that delegation-tree-bound reports (stored at
//! `artifacts/<root_id>/<task_id>/<file>`) are served alongside flat reports
//! (`artifacts/<file>`).

use axum::{
    extract::Path,
    http::{header, StatusCode},
    response::IntoResponse,
};

/// GET /api/v1/artifacts/*path
///
/// Reads a document from the artifacts directory and returns it with the
/// appropriate Content-Type (text/markdown for .md, text/html for .html).
/// `path` may be a flat filename or a nested tree-scoped path
/// (`<root_id>/<task_id>/<filename>`).
pub async fn artifact_handler(Path(path): Path<String>) -> impl IntoResponse {
    let artifacts_dir = crate::dirs::artifacts_dir();

    // Path traversal protection: reject absolute paths, ".." / "." / empty
    // segments, backslashes, and percent-encoded variants, so the joined path
    // can never escape the artifacts root.
    let lower = path.to_lowercase();
    if path.starts_with('/')
        || lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains('\\')
        || path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Access denied".to_string(),
        )
            .into_response();
    }

    let file_path = artifacts_dir.join(std::path::Path::new(&path));

    // Note: we intentionally skip canonicalize() here because on macOS it
    // fails on Unicode filenames due to NFC/NFD normalization differences
    // between the HTTP-decoded path and the filesystem-stored path.
    // The write_report tool already validates paths server-side.

    match tokio::fs::read_to_string(&file_path).await {
        Ok(content) => {
            let mime = if path.ends_with(".html") {
                "text/html; charset=utf-8"
            } else {
                "text/markdown; charset=utf-8"
            };
            (StatusCode::OK, [(header::CONTENT_TYPE, mime)], content).into_response()
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Document not found".to_string(),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_traversal_paths_rejected() {
        for bad in [
            "..",
            "../etc/passwd",
            "/etc/passwd",
            "a/../b.md",
            "a//b.md",
            "%2e%2e/x",
            "%2fetc%2fpasswd",
            "a\\..\\b.md",
            "./a.md",
        ] {
            let resp = artifact_handler(Path(bad.to_string()))
                .await
                .into_response();
            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "should reject {:?}", bad);
        }
    }

    #[tokio::test]
    async fn test_nested_tree_path_served() {
        // A tree-scoped artifact is stored under artifacts/<root>/<task>/<file>
        // and served from the matching URL.
        let dir = crate::dirs::artifacts_dir().join("root-t").join("task-t");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("r.md"), "# hi from tree")
            .await
            .unwrap();

        let resp = artifact_handler(Path("root-t/task-t/r.md".to_string()))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("# hi from tree"));

        let _ = tokio::fs::remove_dir_all(crate::dirs::artifacts_dir().join("root-t")).await;
    }
}
