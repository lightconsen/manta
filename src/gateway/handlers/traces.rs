//! Trace replay HTTP handler.
//!
//! `GET /api/traces/:turn_id` returns a turn's truncated `summary.json` plus
//! its append-only `full.json` (one event per line), so a client can render the
//! full execution timeline for debugging.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;

/// GET /api/traces/:turn_id — summary + full trace for one turn.
pub async fn trace_replay_handler(
    State(_state): State<Arc<GatewayState>>,
    Path(turn_id): Path<String>,
) -> impl IntoResponse {
    // Guard against path traversal: a turn id is a bare uuid-ish segment.
    if turn_id.is_empty()
        || turn_id.contains('/')
        || turn_id.contains('\\')
        || turn_id.contains("..")
    {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "invalid turn_id" })));
    }

    // Turn dirs are `turns/YYYY-MM-DD/<turn_id>/`; the date isn't in the id, so
    // scan the date partitions (bounded — a local personal-assistant store).
    let base = crate::dirs::turns_dir();
    let mut turn_dir = None;
    if let Ok(rd) = std::fs::read_dir(&base) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() && p.join(&turn_id).is_dir() {
                turn_dir = Some(p.join(&turn_id));
                break;
            }
        }
    }

    let Some(dir) = turn_dir else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "turn not found" })));
    };

    let summary = std::fs::read_to_string(dir.join("summary.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let full_events = std::fs::read_to_string(dir.join("full.json"))
        .ok()
        .map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .collect::<Vec<_>>()
        });

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "turn_id": turn_id,
            "summary": summary,
            "full_trace": full_events,
        })),
    )
}
