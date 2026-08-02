//! Tool, skill and approval command handlers.

use super::*;

pub(super) async fn handle_tools(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let verbose = args.trim() == "verbose";
    let tool_names = state.tools.registry.list();

    let text = if verbose {
        let mut lines = vec!["🛠️ **Available Tools**".to_string()];
        for name in &tool_names {
            lines.push(format!("- {}", name));
        }
        lines.join("\n")
    } else {
        format!("🛠️ **Tools** ({} total): {}", tool_names.len(), tool_names.join(", "))
    };

    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
}

pub(super) async fn handle_bash(req: &WsRequest, args: &str) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /bash <command>");
    }

    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(trimmed)
        .output()
        .await
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut lines = vec![format!("💻 `$ {}`", trimmed)];
            if !stdout.is_empty() {
                lines.push("\nstdout:".to_string());
                lines.push(stdout.to_string());
            }
            if !stderr.is_empty() {
                lines.push("\nstderr:".to_string());
                lines.push(stderr.to_string());
            }
            lines.push(format!("\nexit code: {}", output.status.code().unwrap_or(-1)));
            WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
        }
        Err(e) => WsResponse::err(&req.id, "EXEC_FAILED", format!("Failed to execute: {}", e)),
    }
}

pub(super) async fn handle_skill(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        let mgr = state.tools.skills_manager.read().await;
        let skills = mgr.prefilter_skills("", 50, 0).await;
        let names: Vec<String> = skills.into_iter().map(|s| s.name).collect();
        let text = format!("🎯 **Skills** ({} total): {}", names.len(), names.join(", "));
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let name = parts[0];
    let _input = parts.get(1).unwrap_or(&"");

    let mgr = state.tools.skills_manager.read().await;
    match mgr.get_skill(name).await {
        Some(skill) => {
            let text = format!(
                "🎯 **Skill: {}**\n\nVersion: {}\nDescription: {}\nEnabled: {}\nEligible: {}",
                skill.name, skill.version, skill.description, skill.enabled, skill.is_eligible,
            );
            WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
        }
        None => WsResponse::err(&req.id, "SKILL_NOT_FOUND", format!("Skill '{}' not found.", name)),
    }
}

pub(super) async fn handle_allowlist(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let levels = state.auth.command_gate.user_levels();
        if levels.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🛡️ No custom user levels configured." }),
            );
        }
        let mut lines = vec!["🛡️ **User Levels**".to_string()];
        for (user, level) in levels {
            lines.push(format!("- {}: {}", user, level));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
    let sub = parts[0];

    match sub {
        "add" => {
            let user = parts.get(1).unwrap_or(&"").trim();
            let level_str = parts.get(2).unwrap_or(&"user").trim();
            if user.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /allowlist add <user> [chat|user|admin]",
                );
            }
            let level = match level_str {
                "chat" => UserLevel::Chat,
                "admin" => UserLevel::Admin,
                _ => UserLevel::User,
            };
            state.auth.command_gate.set_user_level(user, level);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🛡️ Set {} to level '{}'.", user, level) }),
            )
        }
        "remove" => {
            let user = parts.get(1).unwrap_or(&"").trim();
            if user.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /allowlist remove <user>");
            }
            state.auth.command_gate.clear_user_level(user);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🛡️ Cleared level for '{}'.", user) }),
            )
        }
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /allowlist [list|add|remove]"),
    }
}

pub(super) async fn handle_approve(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let pending = state
            .tools
            .approval_queue
            .list_pending(ApprovalFilter::default())
            .await;
        if pending.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "✅ No pending approvals." }),
            );
        }
        let mut lines = vec![format!("⏳ **Pending Approvals** ({})", pending.len())];
        for pa in &pending {
            lines.push(format!(
                "- {}: {} (risk: {:?}, by: {})",
                pa.id, pa.tool_name, pa.risk_level, pa.requested_by
            ));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let id = parts[0];
    let decision_str = parts.get(1).unwrap_or(&"").trim();

    if id.is_empty() || decision_str.is_empty() {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /approve <id> approve|deny [reason]",
        );
    }

    let decision = match decision_str {
        "approve" | "yes" | "y" => ApprovalDecision::Approve,
        "deny" | "no" | "n" => ApprovalDecision::Deny {
            reason: "Denied via /approve command.".to_string(),
        },
        _ => {
            return WsResponse::err(
                &req.id,
                "INVALID_ARGS",
                "Decision must be 'approve' or 'deny'.",
            );
        }
    };

    if state.tools.approval_queue.resolve(id, decision).await {
        WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("✅ Approval '{}' resolved.", id) }),
        )
    } else {
        WsResponse::err(
            &req.id,
            "NOT_FOUND",
            format!("Approval '{}' not found or already resolved.", id),
        )
    }
}

pub(super) async fn handle_btw(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let question = args.trim();
    if question.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /btw <question>");
    }

    let messages = vec![crate::providers::Message::user(question)];
    match state.infra.model_router.complete_auto(messages, None).await {
        Ok(response) => {
            let text = format!(
                "💡 **Side question** ({}):\n\n{}",
                response.model, response.message.content
            );
            WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
        }
        Err(e) => {
            WsResponse::err(&req.id, "COMPLETION_FAILED", format!("Failed to get response: {}", e))
        }
    }
}
