use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;

// Auth Profile Handlers

#[allow(dead_code)]
pub async fn get_auth_profile_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.get_auth_profile_status(&id).await {
        Some(status) => (StatusCode::OK, Json(serde_json::json!(status))).into_response(),
        None => {
            let error = serde_json::json!({
                "error": format!("No auth profile found for provider '{}'", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn rotate_auth_profile_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.rotate_auth_key(&id).await {
        Ok(_new_key) => {
            let response = serde_json::json!({
                "success": true,
                "provider": id,
                "message": format!("Auth key rotated for provider '{}'", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let error = serde_json::json!({
                "error": format!("Failed to rotate auth key: {}", e),
            });
            (StatusCode::BAD_REQUEST, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn list_auth_profiles_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let profiles = state.infra.model_router.list_auth_profiles().await;
    Json(serde_json::json!({
        "profiles": profiles,
        "count": profiles.len(),
    }))
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

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    #[tokio::test]
    async fn get_auth_profile_unknown_provider_404() {
        let state = state().await;
        let (status, json) = body_json(
            get_auth_profile_handler(Path("openai".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(json["error"].as_str().unwrap().contains("openai"));
    }

    #[tokio::test]
    async fn list_auth_profiles_empty_ok() {
        let state = state().await;
        let (status, json) = body_json(
            list_auth_profiles_handler(State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["count"], 0);
        assert!(json["profiles"].is_array());
    }

    #[tokio::test]
    async fn rotate_auth_key_unknown_provider_400() {
        let state = state().await;
        let (status, json) = body_json(
            rotate_auth_profile_handler(Path("openai".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"]
            .as_str()
            .unwrap()
            .contains("Failed to rotate auth key"));
    }
}
