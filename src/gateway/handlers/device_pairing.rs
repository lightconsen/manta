use std::sync::Arc;
use std::time::SystemTime;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
};
use serde::Deserialize;
use tracing::{info, warn};

use crate::gateway::GatewayState;
use crate::security::device_pairing::DevicePairingStore;

// ── Request types
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DeviceApproveRequest {
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct DeviceRevokeRequest {
    pub device_id: String,
}

// ── Handlers
// ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/device/pairing/pending` — list pending device pairing requests.
pub async fn list_device_pending_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let pending = state.auth.device_pairing_store.list_pending().await;
    Json(serde_json::json!({
        "pending": pending,
        "count": pending.len(),
    }))
}

/// `GET /api/v1/device/pairing/authorized` — list authorized devices.
pub async fn list_device_authorized_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let devices = state.auth.device_pairing_store.list_authorized().await;
    Json(serde_json::json!({
        "devices": devices,
        "count": devices.len(),
    }))
}

/// `POST /api/v1/device/pairing/approve` — approve a device pairing by code.
pub async fn approve_device_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<DeviceApproveRequest>,
) -> impl IntoResponse {
    match state
        .auth
        .device_pairing_store
        .approve(&req.code, Some("admin"))
        .await
    {
        Some(token) => {
            info!("Device pairing approved: code={}", req.code);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "approved",
                    "token": token,
                    "code": req.code,
                })),
            )
                .into_response()
        }
        None => {
            warn!("Device pairing approve failed: code={} not found or expired", req.code);
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Pairing code not found or expired",
                    "code": req.code,
                })),
            )
                .into_response()
        }
    }
}

/// `POST /api/v1/device/pairing/reject` — reject a device pairing by code.
pub async fn reject_device_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<DeviceApproveRequest>,
) -> impl IntoResponse {
    match state.auth.device_pairing_store.reject(&req.code).await {
        Some(_) => {
            info!("Device pairing rejected: code={}", req.code);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "status": "rejected",
                    "code": req.code,
                })),
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Pairing code not found",
                "code": req.code,
            })),
        )
            .into_response(),
    }
}

/// `POST /api/v1/device/pairing/revoke` — revoke an authorized device.
pub async fn revoke_device_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<DeviceRevokeRequest>,
) -> impl IntoResponse {
    let removed = state.auth.device_pairing_store.revoke(&req.device_id).await;
    if removed {
        info!("Device revoked: device_id={}", req.device_id);
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "revoked",
                "device_id": req.device_id,
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Device not found",
                "device_id": req.device_id,
            })),
        )
            .into_response()
    }
}

/// `GET /api/v1/device/pairing/setup/{setup_code}` — decode a base64url setup
/// token back to the pairing code and return the pending request details.
pub async fn setup_device_handler(
    State(state): State<Arc<GatewayState>>,
    Path(setup_code): Path<String>,
) -> impl IntoResponse {
    let code = match DevicePairingStore::decode_setup_code(&setup_code) {
        Some(code) => code,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "Invalid setup code",
                })),
            )
                .into_response()
        }
    };

    let pending = state.auth.device_pairing_store.list_pending().await;
    match pending.into_iter().find(|r| r.code == code) {
        Some(req) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "code": req.code,
                "device_id": req.device_id,
                "display_name": req.display_name,
                "expires_at": req.expires_at.duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            })),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "Pairing code not found or expired",
                "code": code,
            })),
        )
            .into_response(),
    }
}
pub async fn device_qr_handler(
    State(state): State<Arc<GatewayState>>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    // Validate that the code exists in pending requests
    let pending = state.auth.device_pairing_store.list_pending().await;
    let exists = pending.iter().any(|r| r.code == code);

    if !exists {
        return (StatusCode::NOT_FOUND, Html("Pairing code not found or expired".to_string()))
            .into_response();
    }

    let uri = DevicePairingStore::pairing_uri(&code);
    match DevicePairingStore::generate_qr_svg(&uri) {
        Ok(svg) => (StatusCode::OK, [("content-type", "image/svg+xml")], svg).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("Failed to generate QR code: {}", e)),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;
    use crate::security::device_pairing::DeviceAccessResult;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    /// Seed a pending pairing request and return its code.
    async fn seed_pending(state: &GatewayState) -> String {
        match state
            .auth
            .device_pairing_store
            .request_access("dev-1", Some("Phone"), None)
            .await
        {
            DeviceAccessResult::PairingRequired { code } => code,
            _ => panic!("expected a new pending request"),
        }
    }

    #[tokio::test]
    async fn list_pending_empty_then_seeded() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            list_device_pending_handler(State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["count"].as_u64(), Some(0));

        seed_pending(&state).await;
        let (_, body) = body_json(
            list_device_pending_handler(State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(body["count"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn approve_flow_and_list_authorized() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let code = seed_pending(&state).await;

        let (status, body) = body_json(
            approve_device_handler(
                State(state.clone()),
                Json(DeviceApproveRequest { code: "WRONG".into() }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = body_json(
            approve_device_handler(
                State(state.clone()),
                Json(DeviceApproveRequest { code: code.clone() }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"].as_str(), Some("approved"));
        assert!(body["token"].as_str().is_some());

        let (_, body) = body_json(
            list_device_authorized_handler(State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(body["count"].as_u64(), Some(1));
    }

    #[tokio::test]
    async fn reject_flow() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let code = seed_pending(&state).await;

        let (status, _) = body_json(
            reject_device_handler(
                State(state.clone()),
                Json(DeviceApproveRequest { code: "WRONG".into() }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, body) = body_json(
            reject_device_handler(State(state.clone()), Json(DeviceApproveRequest { code }))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"].as_str(), Some("rejected"));
        let (_, body) = body_json(
            list_device_pending_handler(State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(body["count"].as_u64(), Some(0));
    }

    #[tokio::test]
    async fn revoke_flow() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let code = seed_pending(&state).await;

        let (status, _) = body_json(
            revoke_device_handler(
                State(state.clone()),
                Json(DeviceRevokeRequest { device_id: "ghost".into() }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        approve_device_handler(State(state.clone()), Json(DeviceApproveRequest { code })).await;
        let (status, _) = body_json(
            revoke_device_handler(
                State(state),
                Json(DeviceRevokeRequest { device_id: "dev-1".into() }),
            )
            .await
            .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn setup_code_paths() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let code = seed_pending(&state).await;
        let setup = DevicePairingStore::encode_setup_code(&code);

        let (status, _) = body_json(
            setup_device_handler(State(state.clone()), Path("!!not-base64!!".into()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, body) = body_json(
            setup_device_handler(State(state.clone()), Path(setup))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["device_id"].as_str(), Some("dev-1"));

        // Encoded code that decodes but is not pending → 404.
        let ghost = DevicePairingStore::encode_setup_code("ghost-code");
        let (status, _) = body_json(
            setup_device_handler(State(state), Path(ghost))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn qr_paths() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let code = seed_pending(&state).await;

        let (status, _) = body_json(
            device_qr_handler(State(state.clone()), Path("NOPE".into()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let resp = device_qr_handler(State(state), Path(code))
            .await
            .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(resp.headers().get("content-type").is_some());
    }
}
