use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};

use crate::gateway::GatewayState;
use crate::security::device_pairing::DevicePairingStore;

// ── Request types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DeviceApproveRequest {
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct DeviceRevokeRequest {
    pub device_id: String,
}

// ── Handlers ───────────────────────────────────────────────────────────────────

/// `GET /api/v1/device/pairing/pending` — list pending device pairing requests.
pub async fn list_device_pending_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let pending = state.device_pairing_store.list_pending().await;
    Json(serde_json::json!({
        "pending": pending,
        "count": pending.len(),
    }))
}

/// `GET /api/v1/device/pairing/authorized` — list authorized devices.
pub async fn list_device_authorized_handler(
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    let devices = state.device_pairing_store.list_authorized().await;
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
    match state.device_pairing_store.reject(&req.code).await {
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
        None => {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "Pairing code not found",
                    "code": req.code,
                })),
            )
                .into_response()
        }
    }
}

/// `POST /api/v1/device/pairing/revoke` — revoke an authorized device.
pub async fn revoke_device_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<DeviceRevokeRequest>,
) -> impl IntoResponse {
    let removed = state
        .device_pairing_store
        .revoke(&req.device_id)
        .await;
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

/// `GET /api/v1/device/pairing/qr/{code}` — get QR code SVG for a pairing code.
pub async fn device_qr_handler(
    State(state): State<Arc<GatewayState>>,
    Path(code): Path<String>,
) -> impl IntoResponse {
    // Validate that the code exists in pending requests
    let pending = state.device_pairing_store.list_pending().await;
    let exists = pending.iter().any(|r| r.code == code);

    if !exists {
        return (
            StatusCode::NOT_FOUND,
            Html("Pairing code not found or expired".to_string()),
        )
            .into_response();
    }

    let uri = DevicePairingStore::pairing_uri(&code);
    match DevicePairingStore::generate_qr_svg(&uri) {
        Ok(svg) => (
            StatusCode::OK,
            [("content-type", "image/svg+xml")],
            svg,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("Failed to generate QR code: {}", e)),
        )
            .into_response(),
    }
}
