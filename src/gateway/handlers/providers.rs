use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;
use crate::gateway::*;

// Provider Management Handlers

#[allow(dead_code)]
pub async fn list_providers_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let providers = state.infra.model_router.list_providers().await;
    Json(serde_json::json!({
        "providers": providers,
        "count": providers.len(),
    }))
}

#[allow(dead_code)]
pub async fn get_provider_health_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.get_provider_health(&id).await {
        Some(health) => {
            let response = serde_json::json!({
                "provider": id,
                "health": health,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        None => {
            let error = serde_json::json!({
                "error": format!("Provider '{}' not found", id),
                "provider": id,
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn switch_model_handler(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SwitchModelRequest>,
) -> impl IntoResponse {
    match state
        .infra
        .model_router
        .switch_default_model(&body.model)
        .await
    {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Switched to model '{}'", body.model),
                "current_model": body.model,
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn enable_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.enable_provider(&id).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Provider '{}' enabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn disable_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.disable_provider(&id).await {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Provider '{}' disabled", id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn check_provider_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.check_provider_health(&id).await {
        Ok(healthy) => {
            let response = serde_json::json!({
                "provider": id,
                "healthy": healthy,
                "checked_at": chrono::Utc::now().to_rfc3339(),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

// Provider Usage Handlers

#[allow(dead_code)]
pub async fn provider_usage_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let snapshots = state.infra.model_router.all_snapshots_with_quota().await;
    Json(serde_json::json!({
        "providers": snapshots,
        "count": snapshots.len(),
    }))
}

#[allow(dead_code)]
pub async fn provider_usage_by_id_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.infra.model_router.snapshot_with_quota(&id).await {
        Some(snapshot) => (StatusCode::OK, Json(serde_json::json!(snapshot))).into_response(),
        None => {
            let error = serde_json::json!({
                "error": format!("No usage data found for provider '{}'", id),
            });
            (StatusCode::NOT_FOUND, Json(error)).into_response()
        }
    }
}

#[allow(dead_code)]
pub async fn get_fallback_chain_handler(
    Path(model_id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let chain = state.infra.model_router.get_fallback_chain(&model_id).await;
    Json(serde_json::json!({
        "model_id": model_id,
        "fallback_chain": chain,
    }))
}

#[allow(dead_code)]
pub async fn set_fallback_chain_handler(
    Path(model_id): Path<String>,
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<SetFallbackChainRequest>,
) -> impl IntoResponse {
    match state
        .infra
        .model_router
        .set_fallback_chain(&model_id, body.providers)
        .await
    {
        Ok(()) => {
            let response = serde_json::json!({
                "success": true,
                "message": format!("Fallback chain updated for '{}'", model_id),
            });
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": format!("{}", e),
            });
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
    }
}

pub async fn list_models_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let entries = state.infra.model_router.model_catalog.list().await;
    Json(serde_json::json!({
        "models": entries,
    }))
}

#[allow(dead_code)]
pub async fn get_default_model_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let default = state.infra.model_router.get_default_model().await;
    Json(serde_json::json!({
        "default_model": default,
    }))
}

/// `GET /v1/models`
///
/// Returns available concrete model IDs in OpenAI wire format.
pub async fn openai_list_models_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let entries = state.infra.model_router.model_catalog.list().await;
    let data: Vec<_> = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry.id.clone(),
                "object": "model",
                "created": 0,
                "owned_by": entry.provider.clone(),
            })
        })
        .collect();

    Json(serde_json::json!({ "object": "list", "data": data }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;
    use crate::model_router::{ProviderConfig, ProviderType};

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    async fn register_model(state: &GatewayState, model: &str) {
        let config = ProviderConfig {
            provider_type: ProviderType::OpenAi,
            models: vec![model.to_string()],
            default_model: model.to_string(),
            api_key: "test-key".to_string().into(),
            api_keys: vec![],
            auth_profile: None,
            oauth: None,
            base_url: None,
            timeout: std::time::Duration::from_secs(30),
            max_retries: 3,
            retry_delay_ms: 1000,
        };
        state
            .infra
            .model_router
            .add_provider("openai", config)
            .await
            .expect("register provider");
    }

    #[tokio::test]
    async fn list_providers_empty_state() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) =
            body_json(list_providers_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["count"].as_u64(), Some(0));
    }

    #[tokio::test]
    async fn get_provider_health_unknown_404() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            get_provider_health_handler(Path("ghost".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn switch_model_unknown_model_400() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            switch_model_handler(State(state), Json(SwitchModelRequest { model: "ghost".into() }))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["success"].as_bool(), Some(false));
    }

    #[tokio::test]
    async fn switch_model_registered_model_ok() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        register_model(&state, "gpt-4o").await;
        let (status, body) = body_json(
            switch_model_handler(State(state), Json(SwitchModelRequest { model: "gpt-4o".into() }))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn enable_and_disable_unknown_provider_400() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            enable_provider_handler(Path("ghost".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["success"].as_bool(), Some(false));

        let (status, _) = body_json(
            disable_provider_handler(Path("ghost".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn check_provider_unknown_400() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            check_provider_handler(Path("ghost".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["success"].as_bool(), Some(false));
    }

    #[tokio::test]
    async fn provider_usage_empty_state() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) =
            body_json(provider_usage_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["count"].as_u64(), Some(0));
    }

    #[tokio::test]
    async fn provider_usage_by_id_unknown_404() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            provider_usage_by_id_handler(Path("ghost".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn fallback_chain_get_and_set() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        register_model(&state, "gpt-4o").await;

        let (status, body) = body_json(
            get_fallback_chain_handler(Path("gpt-4o".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["model_id"].as_str(), Some("gpt-4o"));
        assert!(body["fallback_chain"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false));

        let (status, body) = body_json(
            set_fallback_chain_handler(
                Path("gpt-4o".into()),
                State(state.clone()),
                Json(SetFallbackChainRequest {
                    providers: vec!["openai".into()],
                }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"].as_bool(), Some(true));
    }

    #[tokio::test]
    async fn set_fallback_chain_unknown_model_400() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, _) = body_json(
            set_fallback_chain_handler(
                Path("ghost".into()),
                State(state),
                Json(SetFallbackChainRequest { providers: vec![] }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_models_empty_state() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) =
            body_json(list_models_handler(State(state)).await.into_response()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["models"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn get_default_model_returns_string() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            get_default_model_handler(State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["default_model"].as_str().is_some());
    }

    #[tokio::test]
    async fn openai_list_models_empty_state() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            openai_list_models_handler(State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["object"].as_str(), Some("list"));
        assert!(body["data"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false));
    }
}
