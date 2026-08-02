//! Model tuning command handlers.

use super::*;

pub(super) async fn handle_model(
    req: &WsRequest,
    _conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        let cfg = state.config.read().await;
        let settings = state.infra.runtime_settings.read().await;
        let override_model = settings.get("model.override").and_then(|v| v.as_str());
        let text = format!(
            "🧠 **Model Status**\n\nDefault: {} (provider: {})\nOverride: {}",
            cfg.model,
            cfg.model_provider,
            override_model.unwrap_or("none")
        );
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("model.override".to_string(), serde_json::json!(trimmed));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🧠 Model override set to '{}'.", trimmed) }),
    )
}

pub(super) async fn handle_think(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let level = args.trim();
    if level.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("think.level")
            .and_then(|v| v.as_str())
            .unwrap_or("medium");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🧠 Thinking level: {}", current) }),
        );
    }
    let valid = ["off", "minimal", "low", "medium", "high"];
    if !valid.contains(&level) {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            format!("Valid levels: {}", valid.join(", ")),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("think.level".to_string(), serde_json::json!(level));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🧠 Thinking level set to '{}'.", level) }),
    )
}

pub(super) async fn handle_verbose(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("verbose.mode")
            .and_then(|v| v.as_str())
            .unwrap_or("off");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔊 Verbose mode: {}", current) }),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("verbose.mode".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🔊 Verbose mode set to '{}'.", mode) }),
    )
}

pub(super) async fn handle_trace(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("trace.enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔍 Plugin trace: {}", if current { "on" } else { "off" }) }),
        );
    }
    let enabled = mode == "on";
    state.infra.plugin_manager.set_trace_enabled(enabled);
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("trace.enabled".to_string(), serde_json::json!(enabled));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("🔍 Plugin trace {}.", if enabled { "enabled" } else { "disabled" }) }),
    )
}

pub(super) async fn handle_fast(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() || mode == "status" {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("fast.mode")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let active_model = settings
            .get("fast.active_model")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({
                "text": format!(
                    "⚡ Fast mode: {} (active model: {})",
                    if current { "on" } else { "off" },
                    active_model
                )
            }),
        );
    }
    let enabled = mode == "on";

    if enabled {
        // Resolve the fast model alias and read the current default model once.
        let fast_model = state.infra.model_router.resolve_alias("fast").await;
        let current_model = state.config.read().await.model.clone();
        let active_model = fast_model.clone().unwrap_or_else(|| current_model.clone());

        {
            let mut settings = state.infra.runtime_settings.write().await;
            settings.insert("fast.original_model".to_string(), serde_json::json!(current_model));
            settings.insert("fast.active_model".to_string(), serde_json::json!(active_model));
            settings.insert("fast.mode".to_string(), serde_json::json!(true));
        }

        if let Some(fast_model) = fast_model {
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚡ Fast mode enabled. Model switched to '{}'.", fast_model) }),
            )
        } else {
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "⚡ Fast mode enabled (no fast alias configured, using current model)." }),
            )
        }
    } else {
        // Restore the original default model and clear fast state in a single
        // runtime_settings write. Config is no longer mutated directly.
        let (original, active_model) = {
            let settings = state.infra.runtime_settings.read().await;
            (
                settings
                    .get("fast.original_model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                settings
                    .get("fast.active_model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            )
        };

        {
            let mut settings = state.infra.runtime_settings.write().await;
            settings.insert("fast.mode".to_string(), serde_json::json!(false));
            if let Some(ref orig) = original {
                settings.insert("fast.active_model".to_string(), serde_json::json!(orig));
            } else {
                settings.remove("fast.active_model");
            }
        }

        WsResponse::ok(
            &req.id,
            serde_json::json!({
                "text": format!(
                    "⚡ Fast mode disabled. Model restored to '{}'.",
                    active_model.unwrap_or_else(|| "default".to_string())
                )
            }),
        )
    }
}

pub(super) async fn handle_reasoning(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("reasoning.visibility")
            .and_then(|v| v.as_str())
            .unwrap_or("on");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("💭 Reasoning visibility: {}", current) }),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("reasoning.visibility".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("💭 Reasoning visibility set to '{}'.", mode) }),
    )
}

pub(super) async fn handle_queue(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let mode = args.trim();
    if mode.is_empty() {
        let settings = state.infra.runtime_settings.read().await;
        let current = settings
            .get("queue.mode")
            .and_then(|v| v.as_str())
            .unwrap_or("steer");
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("📥 Queue mode: {}", current) }),
        );
    }
    let valid = ["steer", "interrupt", "followup"];
    if !valid.contains(&mode) {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            format!("Valid queue modes: {}", valid.join(", ")),
        );
    }
    let mut settings = state.infra.runtime_settings.write().await;
    settings.insert("queue.mode".to_string(), serde_json::json!(mode));
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": format!("📥 Queue mode set to '{}'.", mode) }),
    )
}
