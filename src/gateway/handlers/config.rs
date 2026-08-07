use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
};
use tracing::info;

use crate::gateway::GatewayState;
use crate::gateway::*;

/// Persist a `GatewayConfig` to disk atomically.
///
/// Writes to a temporary file next to `config_path` and renames it into place
/// so readers never observe a partially-written config.
pub(crate) async fn persist_config_atomic(
    config: &GatewayConfig,
    config_path: &Path,
) -> Result<(), String> {
    let toml_str =
        toml::to_string_pretty(config).map_err(|e| format!("TOML serialization failed: {}", e))?;

    let tmp_path = config_path.with_extension("toml.tmp");
    tokio::fs::write(&tmp_path, toml_str)
        .await
        .map_err(|e| format!("Failed to write temporary config file: {}", e))?;
    tokio::fs::rename(&tmp_path, config_path)
        .await
        .map_err(|e| format!("Failed to atomically replace config file: {}", e))?;

    Ok(())
}

#[allow(dead_code)]
/// Get current gateway configuration
pub async fn get_config_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let config = state.config.read().await;
    match serde_json::to_value(&*config) {
        Ok(json) => (StatusCode::OK, Json(json)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Serialization failed: {}", e)})),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// Update gateway configuration and persist to disk
pub async fn put_config_handler(
    State(state): State<Arc<GatewayState>>,
    Json(new_config): Json<GatewayConfig>,
) -> impl IntoResponse {
    let config_path = match state.config_path.clone() {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(
                    serde_json::json!({"error": "No config file path configured — cannot persist changes"}),
                ),
            )
                .into_response();
        }
    };

    // Write to disk atomically
    if let Err(e) = persist_config_atomic(&new_config, &config_path).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e})))
            .into_response();
    }

    // Update in-memory config
    {
        let mut config = state.config.write().await;
        *config = Arc::new(new_config);
    }

    info!("Config updated and persisted to {:?}", config_path);

    state
        .auth
        .audit_log
        .log(
            crate::security::runtime_audit::AuditEventType::ConfigChange,
            "admin",
            "gateway",
            true,
            format!("Config updated and persisted to {}", config_path.display()),
            Some(serde_json::json!({"path": config_path.to_string_lossy()})),
        )
        .await;

    (
        StatusCode::OK,
        Json(serde_json::json!({"status": "updated", "path": config_path.to_string_lossy()})),
    )
        .into_response()
}

#[allow(dead_code)]
/// Validate a configuration without persisting it
pub async fn validate_config_handler(Json(config): Json<GatewayConfig>) -> impl IntoResponse {
    // Basic validation: try to serialize and deserialize as TOML
    match toml::to_string(&config) {
        Ok(toml_str) => {
            match toml::from_str::<GatewayConfig>(&toml_str) {
                Ok(_) => (
                    StatusCode::OK,
                    Json(serde_json::json!({"valid": true, "message": "Configuration is valid"})),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"valid": false, "error": format!("TOML deserialization failed: {}", e)})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"valid": false, "error": format!("TOML serialization failed: {}", e)})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    #[tokio::test]
    async fn get_config_serializes() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) =
            body_json(get_config_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["model"].as_str().is_some());
        assert!(body["model_provider"].as_str().is_some());
    }

    #[tokio::test]
    async fn put_config_without_path_returns_501() {
        // make_test_state leaves config_path None → handler must 501.
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            put_config_handler(State(state), Json(GatewayConfig::default()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        assert!(body["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn validate_config_accepts_default() {
        let (status, body) = body_json(
            validate_config_handler(Json(GatewayConfig::default()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["valid"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn persist_config_atomic_roundtrip() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let cfg = GatewayConfig::default();
        persist_config_atomic(&cfg, &path).await.expect("persist");
        let on_disk = tokio::fs::read_to_string(&path).await.expect("read");
        assert!(on_disk.contains("model_provider"), "TOML should round-trip");
        let reparsed: GatewayConfig = toml::from_str(&on_disk).expect("reparse");
        assert_eq!(reparsed.model, cfg.model);
    }

    #[tokio::test]
    async fn persist_config_atomic_errors_on_bad_dir() {
        let err = persist_config_atomic(
            &GatewayConfig::default(),
            std::path::Path::new("/nonexistent-dir-xyz/config.toml"),
        )
        .await;
        assert!(err.is_err(), "write into missing dir must fail");
    }
}
