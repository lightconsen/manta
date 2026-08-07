use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;
use crate::gateway::*;

#[allow(dead_code)]
/// `GET /api/settings` — list all runtime key/value settings.
pub async fn list_settings_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let settings = state.infra.runtime_settings.read().await.clone();
    Json(settings)
}

#[allow(dead_code)]
/// `POST /api/settings` — upsert a runtime setting.
pub async fn set_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<SetSettingRequest>,
) -> impl IntoResponse {
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert(req.key.clone(), req.value.clone());
    Json(serde_json::json!({ "ok": true, "key": req.key }))
}

#[allow(dead_code)]
/// `GET /api/settings/:key` — read one setting by key.
pub async fn get_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let settings = state.infra.runtime_settings.read().await;
    match settings.get(&key) {
        Some(val) => Json(serde_json::json!({ "key": key, "value": val })).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Setting '{}' not found", key) })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `DELETE /api/settings/:key` — remove one setting.
pub async fn delete_setting_handler(
    State(state): State<Arc<GatewayState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let mut settings = state.infra.runtime_settings.write().await;
    if settings.remove(&key).is_some() {
        Json(serde_json::json!({ "ok": true, "key": key })).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Setting '{}' not found", key) })),
        )
            .into_response()
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
    async fn list_settings_returns_all() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        state
            .infra
            .runtime_settings
            .write()
            .await
            .insert("verbose.mode".into(), serde_json::json!("full"));
        let (status, body) =
            body_json(list_settings_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["verbose.mode"].as_str(), Some("full"));
    }

    #[tokio::test]
    async fn set_setting_upserts() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let req = SetSettingRequest {
            key: "queue.mode".into(),
            value: serde_json::json!("interrupt"),
        };
        let (status, body) = body_json(
            set_setting_handler(State(state.clone()), Json(req))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"].as_bool(), Some(true));
        assert_eq!(
            state.infra.runtime_settings.read().await.get("queue.mode"),
            Some(&serde_json::json!("interrupt"))
        );
    }

    #[tokio::test]
    async fn get_setting_found_and_missing() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        state
            .infra
            .runtime_settings
            .write()
            .await
            .insert("think.level".into(), serde_json::json!(2));
        let (status, body) = body_json(
            get_setting_handler(State(state.clone()), Path("think.level".into()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["value"].as_u64(), Some(2));

        let (status, _) = body_json(
            get_setting_handler(State(state), Path("nope".into()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_setting_existing_and_missing() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        state
            .infra
            .runtime_settings
            .write()
            .await
            .insert("trace.enabled".into(), serde_json::json!(true));
        let (status, body) = body_json(
            delete_setting_handler(State(state.clone()), Path("trace.enabled".into()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["ok"].as_bool().unwrap_or_default());
        assert!(!state
            .infra
            .runtime_settings
            .read()
            .await
            .contains_key("trace.enabled"));

        let (status, _) = body_json(
            delete_setting_handler(State(state), Path("nope".into()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
