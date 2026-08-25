//! Artifact serving handler
//!
//! Serves `write_report` documents for the document preview feature. Reports
//! live under each producing agent's own workspace (`<workspace>/artifacts/`);
//! reports written before that move stay in the legacy `~/.syscity/artifacts/`
//! directory and remain reachable via their old URLs.
//!
//! URL shapes (the route is a wildcard):
//! - `/api/v1/artifacts/@<agent_id>/<file>` — an agent's workspace artifacts
//!   (`@default` = the shared default workspace).
//! - `/api/v1/artifacts/@<agent_id>/<root>/<task>/<file>` — delegation-scoped
//!   report inside that agent's workspace.
//! - `/api/v1/artifacts/[<root>/<task>/]<file>` — legacy global artifacts dir.

use axum::{
    extract::{Path, Query},
    http::{header, StatusCode},
    response::IntoResponse,
};

#[derive(Debug, serde::Deserialize)]
pub struct ArtifactExportQuery {
    /// Export conversion target (currently only "pptx" for slides canvases).
    to: Option<String>,
}

/// Resolve a canvas `<img src>` to bytes, relative to the artifact's own
/// directory. Anything escaping that directory is skipped (the converter
/// drops the image rather than failing the export).
struct ArtifactImageResolver {
    base: std::path::PathBuf,
}

impl crate::office::slides::ImageResolver for ArtifactImageResolver {
    fn resolve(&self, src: &str) -> Option<(Vec<u8>, String)> {
        if src.starts_with('/') || src.contains("..") || src.starts_with("http") {
            return None;
        }
        let path = self.base.join(src);
        let canon = path.canonicalize().ok()?;
        if !canon.starts_with(&self.base) {
            return None;
        }
        let bytes = std::fs::read(&canon).ok()?;
        let format = canon
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png")
            .trim_start_matches('.')
            .to_string();
        Some((bytes, format))
    }
}

/// Convert a slides-canvas artifact to a real .pptx download.
async fn export_as_pptx(
    file_path: std::path::PathBuf,
    filename: String,
) -> axum::response::Response {
    let html = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::NOT_FOUND,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                "Document not found".to_string(),
            )
                .into_response();
        }
    };

    // Conversion is CPU-bound and synchronous (HTML parse + OOXML zip).
    let base = file_path.parent().map(|p| p.to_path_buf());
    let title = filename.clone();
    let result = tokio::task::spawn_blocking(move || {
        let resolver = ArtifactImageResolver { base: base.unwrap_or_default() };
        crate::office::slides::canvas_html_to_pptx(&html, &title, &resolver)
    })
    .await;

    match result {
        Ok(Ok(bytes)) => {
            let download_name = filename
                .trim_end_matches(".html")
                .trim_end_matches(".md")
                .to_string()
                + ".pptx";
            (
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                            .to_string(),
                    ),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{download_name}\""),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("Conversion failed: {e}"),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("Conversion panicked: {e}"),
        )
            .into_response(),
    }
}

/// Resolve a request path to its on-disk location, honoring the `@owner`
/// prefix for agent-workspace artifacts and falling back to the legacy global
/// directory. Returns `None` for unsafe paths.
fn resolve_artifact_path(path: &str) -> Option<std::path::PathBuf> {
    // Path traversal protection: reject absolute paths, ".." / "." / empty
    // segments, backslashes, and percent-encoded variants, so the joined path
    // can never escape its root.
    let lower = path.to_lowercase();
    if path.starts_with('/')
        || lower.contains("%2e")
        || lower.contains("%2f")
        || path.contains('\\')
        || path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..")
    {
        return None;
    }

    let (root, rel) = if let Some(rest) = path.strip_prefix('@') {
        let (owner, rel) = rest.split_once('/')?;
        // The owner segment must be an agent id — the same charset the
        // write side allows (alphanumeric, '-', '_').
        if owner.is_empty()
            || !owner
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return None;
        }
        let ws = if owner == "default" {
            crate::dirs::workspace_data_dir()
        } else {
            crate::dirs::agent_workspace_dir(owner)
        };
        (ws.join("artifacts"), rel)
    } else {
        (crate::dirs::artifacts_dir(), path)
    };

    Some(root.join(std::path::Path::new(rel)))
}

/// GET /api/v1/artifacts/*path  (+ `?to=pptx` for on-demand conversion)
///
/// Reads a document from the resolved artifacts location and returns it with
/// the appropriate Content-Type (text/markdown for .md, text/html for .html).
/// With `?to=pptx` the document must be a slides canvas and the response is
/// the converted .pptx download instead of the source.
pub async fn artifact_handler(
    Path(path): Path<String>,
    Query(query): Query<ArtifactExportQuery>,
) -> impl IntoResponse {
    let Some(file_path) = resolve_artifact_path(&path) else {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "Access denied".to_string(),
        )
            .into_response();
    };

    // Note: we intentionally skip canonicalize() here because on macOS it
    // fails on Unicode filenames due to NFC/NFD normalization differences
    // between the HTTP-decoded path and the filesystem-stored path.
    // The write_report tool already validates paths server-side.

    if query.to.as_deref() == Some("pptx") {
        let filename = path.rsplit('/').next().unwrap_or("slides.html").to_string();
        return export_as_pptx(file_path, filename).await;
    }
    if let Some(to) = &query.to {
        return (
            StatusCode::BAD_REQUEST,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("Unsupported export target '{to}' (supported: pptx)"),
        )
            .into_response();
    }

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
            let resp =
                artifact_handler(Path(bad.to_string()), Query(ArtifactExportQuery { to: None }))
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

        let resp = artifact_handler(
            Path("root-t/task-t/r.md".to_string()),
            Query(ArtifactExportQuery { to: None }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("# hi from tree"));

        let _ = tokio::fs::remove_dir_all(crate::dirs::artifacts_dir().join("root-t")).await;
    }

    #[tokio::test]
    async fn test_agent_workspace_path_served() {
        // Agent-workspace artifacts are addressed with an `@<owner>` prefix
        // and served from that workspace's artifacts dir.
        let dir = crate::dirs::workspace_data_dir().join("artifacts");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("ws-test.md"), "# hi from workspace")
            .await
            .unwrap();

        let resp = artifact_handler(
            Path("@default/ws-test.md".to_string()),
            Query(ArtifactExportQuery { to: None }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("# hi from workspace"));

        let _ = tokio::fs::remove_file(dir.join("ws-test.md")).await;
    }

    #[tokio::test]
    async fn test_owner_segment_validated() {
        for bad in ["@../x.md", "@a.b/x.md", "@default"] {
            let resp =
                artifact_handler(Path(bad.to_string()), Query(ArtifactExportQuery { to: None }))
                    .await
                    .into_response();
            assert_ne!(resp.status(), StatusCode::OK, "should not serve {:?}", bad);
        }
    }

    #[tokio::test]
    async fn test_slides_export_returns_pptx() {
        let dir = crate::dirs::artifacts_dir();
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(
            dir.join("export-deck.html"),
            r#"<div class="slide"><div style="position:absolute;left:80px;top:60px;width:1120px">标题页</div></div>"#,
        )
        .await
        .unwrap();

        let resp = artifact_handler(
            Path("export-deck.html".to_string()),
            Query(ArtifactExportQuery { to: Some("pptx".to_string()) }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(ct.contains("presentationml"), "content-type: {ct}");
        let body = axum::body::to_bytes(resp.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        // A pptx is a zip package.
        assert_eq!(&body[..2], b"PK");

        let _ = tokio::fs::remove_file(dir.join("export-deck.html")).await;
    }

    #[tokio::test]
    async fn test_export_rejects_unknown_target() {
        let resp = artifact_handler(
            Path("x.md".to_string()),
            Query(ArtifactExportQuery { to: Some("pdf".to_string()) }),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
