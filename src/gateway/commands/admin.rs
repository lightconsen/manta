//! Admin and plugin command handlers.

use super::*;

pub(super) async fn handle_config(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let cfg = state.config.read().await;
        let settings = state.infra.runtime_settings.read().await;
        let mut lines = vec!["⚙️ **Config**".to_string()];
        lines.push(format!("Model: {} (provider: {})", cfg.model, cfg.model_provider));
        lines.push(format!("Host: {}:{}", cfg.host, cfg.port));
        if !settings.is_empty() {
            lines.push("\nRuntime settings:".to_string());
            for (k, v) in settings.iter() {
                lines.push(format!("  {} = {}", k, v));
            }
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
    let sub = parts[0];

    match sub {
        "get" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config get <key>");
            }
            let settings = state.infra.runtime_settings.read().await;
            match settings.get(key) {
                Some(v) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("⚙️ {} = {}", key, v) }),
                ),
                None => WsResponse::err(&req.id, "NOT_FOUND", format!("Key '{}' not found.", key)),
            }
        }
        "set" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            let val = parts.get(2).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /config set <key> <value>",
                );
            }
            let mut settings = state.infra.runtime_settings.write().await;
            let json_val = serde_json::from_str(val).unwrap_or_else(|_| serde_json::json!(val));
            settings.insert(key.to_string(), json_val.clone());
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚙️ Set {} = {}", key, json_val) }),
            )
        }
        "unset" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config unset <key>");
            }
            let mut settings = state.infra.runtime_settings.write().await;
            settings.remove(key);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("⚙️ Removed key '{}'.", key) }),
            )
        }
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /config [show|get|set|unset]"),
    }
}

pub(super) async fn handle_plugins(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "list" {
        let plugins = state.infra.plugin_manager.list_plugins().await;
        if plugins.is_empty() {
            return WsResponse::ok(&req.id, serde_json::json!({ "text": "🔌 No plugins loaded." }));
        }
        let mut lines = vec![format!("🔌 **Plugins** ({} total)", plugins.len())];
        for p in &plugins {
            lines.push(format!(
                "- {} ({}) — {} [{}]",
                p.name(),
                p.id(),
                p.manifest.description,
                if p.enabled { "enabled" } else { "disabled" }
            ));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "enable" => {
            if rest.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /plugins enable <id>");
            }
            match state.infra.plugin_manager.set_enabled(rest, true).await {
                Ok(()) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🔌 Plugin '{}' enabled.", rest) }),
                ),
                Err(e) => WsResponse::err(&req.id, "PLUGIN_ERROR", format!("{}", e)),
            }
        }
        "disable" => {
            if rest.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /plugins disable <id>");
            }
            match state.infra.plugin_manager.set_enabled(rest, false).await {
                Ok(()) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🔌 Plugin '{}' disabled.", rest) }),
                ),
                Err(e) => WsResponse::err(&req.id, "PLUGIN_ERROR", format!("{}", e)),
            }
        }
        _ => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🔌 Plugin command '{}' not yet implemented.", sub) }),
        ),
    }
}

pub(super) async fn handle_mcp(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let servers = state.tools.mcp_manager.list_servers().await;
        if servers.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🔌 No MCP servers connected." }),
            );
        }
        let text = format!("🔌 **MCP Servers** ({}): {}", servers.len(), servers.join(", "));
        return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
    }

    let mut tokens = trimmed.split_whitespace();
    let sub = tokens.next().unwrap_or("");

    match sub {
        "connect" => {
            let rest: Vec<&str> = tokens.collect();
            if rest.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp connect <server_id> [command] [args...]",
                );
            }
            let server_id = rest[0].to_string();
            let config = if rest.len() > 1 {
                let base = state
                    .config
                    .read()
                    .await
                    .mcp
                    .servers
                    .get(&server_id)
                    .cloned()
                    .unwrap_or_default();
                let command = rest[1].to_string();
                let args = rest[2..].iter().map(|s| s.to_string()).collect();
                McpServerConfig {
                    command: Some(command),
                    args,
                    ..base
                }
            } else {
                match state
                    .config
                    .read()
                    .await
                    .mcp
                    .servers
                    .get(&server_id)
                    .cloned()
                {
                    Some(cfg) => cfg,
                    None => {
                        return WsResponse::err(
                            &req.id,
                            "INVALID_ARGS",
                            "Usage: /mcp connect <server_id> [command] [args...]",
                        );
                    }
                }
            };

            match state
                .tools
                .mcp_manager
                .connect(&server_id, config.clone())
                .await
            {
                Ok(tools) => {
                    if let Some(client_arc) = state.tools.mcp_manager.get_client(&server_id).await {
                        let max_tools = if config.max_tools == 0 {
                            tools.len()
                        } else {
                            config.max_tools.min(tools.len())
                        };
                        for tool in tools.iter().take(max_tools) {
                            let wrapper =
                                Arc::new(McpToolWrapper::new(client_arc.clone(), &server_id, tool));
                            state.tools.registry.register_dynamic(wrapper);
                        }
                    }
                    let text = format!(
                        "🔌 Connected MCP server '{}' ({} tool{} registered).",
                        server_id,
                        tools.len(),
                        if tools.len() == 1 { "" } else { "s" }
                    );
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": text,
                            "server_id": server_id,
                            "tools": tools.len(),
                        }),
                    )
                }
                Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
            }
        }
        "disconnect" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp disconnect <server_id>",
                );
            }
            match state.tools.mcp_manager.disconnect(server_id).await {
                Ok(()) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🔌 MCP server '{}' disconnected.", server_id) }),
                ),
                Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
            }
        }
        "tools" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /mcp tools <server_id>");
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => {
                    let client = client_arc.read().await;
                    let tools: Vec<serde_json::Value> = client
                        .get_tools()
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "name": t.name,
                                "description": t.description,
                            })
                        })
                        .collect();
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!("🔌 {} tool(s) on '{}'", tools.len(), server_id),
                            "server_id": server_id,
                            "tools": tools,
                        }),
                    )
                }
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "resources" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp resources <server_id>",
                );
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => match client_arc.read().await.list_resources().await {
                    Ok(resources) => {
                        let items: Vec<serde_json::Value> = resources
                            .iter()
                            .map(|r| {
                                serde_json::json!({
                                    "uri": r.uri,
                                    "name": r.name,
                                    "description": r.description,
                                    "mime_type": r.mime_type,
                                })
                            })
                            .collect();
                        WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!(
                                    "🔌 {} resource(s) on '{}'",
                                    items.len(),
                                    server_id
                                ),
                                "server_id": server_id,
                                "resources": items,
                            }),
                        )
                    }
                    Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                },
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "prompts" => {
            let server_id = tokens.next().unwrap_or("");
            if server_id.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /mcp prompts <server_id>");
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => match client_arc.read().await.list_prompts().await {
                    Ok(prompts) => {
                        let items: Vec<serde_json::Value> = prompts
                            .iter()
                            .map(|p| {
                                serde_json::json!({
                                    "name": p.name,
                                    "description": p.description,
                                    "arguments": p.arguments,
                                })
                            })
                            .collect();
                        WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!(
                                    "🔌 {} prompt(s) on '{}'",
                                    items.len(),
                                    server_id
                                ),
                                "server_id": server_id,
                                "prompts": items,
                            }),
                        )
                    }
                    Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                },
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "call" => {
            let server_id = tokens.next().unwrap_or("");
            let tool_name = tokens.next().unwrap_or("");
            let json_args = tokens.collect::<Vec<&str>>().join(" ");
            if server_id.is_empty() || tool_name.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp call <server_id> <tool_name> [json_args]",
                );
            }
            let params = if json_args.is_empty() {
                serde_json::json!({})
            } else {
                match serde_json::from_str(&json_args) {
                    Ok(v) => v,
                    Err(e) => {
                        return WsResponse::err(
                            &req.id,
                            "INVALID_ARGS",
                            format!("Invalid JSON arguments: {}", e),
                        );
                    }
                }
            };
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => {
                    match client_arc.read().await.call_tool(tool_name, params).await {
                        Ok(result) => WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!("🔌 Tool '{}' returned result.", tool_name),
                                "server_id": server_id,
                                "tool": tool_name,
                                "result": result,
                            }),
                        ),
                        Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                    }
                }
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        "read" => {
            let server_id = tokens.next().unwrap_or("");
            let uri = tokens.next().unwrap_or("");
            if server_id.is_empty() || uri.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /mcp read <server_id> <uri>",
                );
            }
            match state.tools.mcp_manager.get_client(server_id).await {
                Some(client_arc) => match client_arc.read().await.read_resource(uri).await {
                    Ok(contents) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!(
                                "🔌 Read {} content fragment(s) from '{}'.",
                                contents.len(),
                                uri
                            ),
                            "server_id": server_id,
                            "uri": uri,
                            "contents": contents,
                        }),
                    ),
                    Err(e) => WsResponse::err(&req.id, "MCP_ERROR", format!("{}", e)),
                },
                None => WsResponse::err(
                    &req.id,
                    "MCP_ERROR",
                    format!("MCP server '{}' is not connected.", server_id),
                ),
            }
        }
        _ => WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /mcp [show|connect|disconnect|tools|resources|prompts|call|read]",
        ),
    }
}

pub(super) async fn handle_debug(
    req: &WsRequest,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "show" {
        let settings = state.infra.runtime_settings.read().await;
        let mut lines = vec!["🐛 **Debug Overrides**".to_string()];
        if settings.is_empty() {
            lines.push("No runtime overrides set.".to_string());
        } else {
            for (k, v) in settings.iter() {
                lines.push(format!("  {} = {}", k, v));
            }
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
    let sub = parts[0];

    match sub {
        "set" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            let val = parts.get(2).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /debug set <key> <value>");
            }
            let mut settings = state.infra.runtime_settings.write().await;
            let json_val = serde_json::from_str(val).unwrap_or_else(|_| serde_json::json!(val));
            settings.insert(key.to_string(), json_val.clone());
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🐛 Set {} = {}", key, json_val) }),
            )
        }
        "unset" => {
            let key = parts.get(1).unwrap_or(&"").trim();
            if key.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /debug unset <key>");
            }
            let mut settings = state.infra.runtime_settings.write().await;
            settings.remove(key);
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("🐛 Removed key '{}'.", key) }),
            )
        }
        "reset" => {
            let mut settings = state.infra.runtime_settings.write().await;
            settings.clear();
            WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🐛 All runtime overrides cleared." }),
            )
        }
        _ => WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /debug [show|set|unset|reset]"),
    }
}

pub(super) async fn handle_restart(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let state_for_restart = state.clone();
    let restart_handle = tokio::spawn(async move {
        // Give the response a moment to be sent, then perform graceful shutdown.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let shutdown_token = state_for_restart.shutdown_token.clone();
        shutdown_token.cancel();

        match tokio::time::timeout(
            std::time::Duration::from_secs(30),
            crate::gateway::lifecycle::stop_gateway(&shutdown_token, &state_for_restart),
        )
        .await
        {
            Ok(Ok(())) => info!("Graceful shutdown completed for restart"),
            Ok(Err(e)) => warn!("Graceful shutdown failed during restart: {}", e),
            Err(_) => warn!("Graceful shutdown timed out during restart"),
        }

        std::process::exit(0);
    });
    state
        .task_registry
        .insert_join("system:restart", restart_handle)
        .await;
    WsResponse::ok(
        &req.id,
        serde_json::json!({ "text": "🔄 Gateway restart initiated. The process will exit gracefully." }),
    )
}
