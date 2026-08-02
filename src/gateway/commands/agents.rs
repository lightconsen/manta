//! Agent control command handlers.

use super::*;

pub(super) async fn handle_goal(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();

    // Parse subcommands (e.g., /goal cancel <id>).
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    if first_word == "cancel" || first_word == "list" {
        let rest = trimmed.split_once(' ').map(|x| x.1).unwrap_or("").trim();
        match first_word {
            "cancel" => {
                if rest.is_empty() {
                    return WsResponse::err(
                        &req.id,
                        "INVALID_ARGS",
                        "Usage: /goal cancel <goal_id>",
                    );
                }
                let cancelled = {
                    let mut cancellers = state.agents.goal_cancellers.write().await;
                    if let Some(token) = cancellers.remove(rest) {
                        token.cancel();
                        true
                    } else {
                        false
                    }
                };
                if cancelled {
                    return WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "text": format!("🎯 Goal `{}` cancelled.", rest) }),
                    );
                } else {
                    return WsResponse::err(
                        &req.id,
                        "GOAL_NOT_FOUND",
                        format!("Goal `{}` not found or already completed.", rest),
                    );
                }
            }
            "list" => {
                let cancellers = state.agents.goal_cancellers.read().await;
                let ids: Vec<&String> = cancellers.keys().collect();
                if ids.is_empty() {
                    return WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "text": "🎯 No active goals." }),
                    );
                }
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "text": format!("🎯 **Active Goals**\n\n{}", ids.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")),
                        "goals": ids,
                    }),
                );
            }
            _ => unreachable!(),
        }
    }

    let description = trimmed;
    if description.is_empty() {
        return WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /goal <description> [--max-rounds N]",
        );
    }

    // Parse optional --max-rounds flag.
    let mut description = trimmed.to_string();
    let mut max_rounds: usize = 5;
    if let Some(pos) = trimmed.rfind("--max-rounds") {
        let before = &trimmed[..pos].trim();
        let rest = trimmed[pos..].trim();
        if let Some(val_str) = rest.split_whitespace().nth(1) {
            if let Ok(n) = val_str.parse::<usize>() {
                max_rounds = n.max(1);
                description = before.to_string();
            }
        }
    }

    if description.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Description required");
    }

    // Resolve the real session_id from the connection's subscriptions.
    let session_id = conn.read().await.subscriptions.first().cloned();

    // Parse the goal description into structured conditions using the LLM.
    let plan = match crate::goal::GoalPlan::parse_with_llm(
        &state.infra.model_router,
        &description,
        Some(max_rounds),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            return WsResponse::err(
                &req.id,
                "GOAL_PARSE_FAILED",
                format!("Failed to parse goal: {}", e),
            );
        }
    };

    let goal_id = format!("goal_{}", uuid::Uuid::new_v4());
    let sid = session_id.unwrap_or_else(|| "unknown".to_string());

    // Create event channel between GoalRunner and gateway.
    let (goal_tx, mut goal_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_tx = state.events.tx.clone();
    let gid = goal_id.clone();
    let s_for_relay = sid.clone();

    // Spawn event relay: GoalEvent → GatewayEvent.
    tokio::spawn(async move {
        while let Some(goal_event) = goal_rx.recv().await {
            let gw_event = crate::gateway::GatewayEvent::GoalProgress {
                goal_id: gid.clone(),
                session_id: s_for_relay.clone(),
                event: goal_event,
            };
            if let Err(e) = event_tx.send(gw_event) {
                warn!("[goal] Failed to broadcast event: {}", e);
                break;
            }
        }
    });

    // Create goal store for persistence (checkpoint after each round).
    let goal_store = crate::goal::persist::shared_store();

    // Create the cancel token and register it for /goal cancel.
    let runner = crate::goal::GoalRunner::new(
        &goal_id,
        &sid,
        plan,
        state.tools.registry.clone(),
        state.infra.model_router.clone(),
        goal_tx,
    )
    .with_store(goal_store.clone());
    let cancel_token = runner.cancel_token();
    {
        let mut cancellers = state.agents.goal_cancellers.write().await;
        cancellers.insert(goal_id.clone(), cancel_token);
    }

    // Spawn GoalRunner as background task — remove cancellers entry when done.
    let gid2 = goal_id.clone();
    let cancellers = state.agents.goal_cancellers.clone();
    tokio::spawn(async move {
        runner.run().await;
        // Clean up cancellers entry on completion.
        let mut c = cancellers.write().await;
        c.remove(&gid2);
    });

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "text": format!("🎯 Goal started: {}\nID: {}\nMax rounds: {}\n\nGoal events will appear in this session.", description, goal_id, max_rounds),
            "goal_id": goal_id,
        }),
    )
}

pub(super) async fn handle_subagents(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();

    if trimmed.is_empty() || trimmed == "list" {
        let session_id = conn.read().await.subscriptions.first().cloned();
        let handles = if let Some(ref sid) = session_id {
            state
                .agents
                .acp
                .list_session_subagents(&AcpSessionId(sid.clone()))
                .await
        } else {
            state.agents.acp.list_subagents().await
        };

        if handles.is_empty() {
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": "🤖 No subagents found." }),
            );
        }

        let mut lines = vec![format!("🤖 **Subagents** ({} total)", handles.len())];
        for h in &handles {
            lines.push(format!(
                "- `{}` — status: `{:?}`, mode: `{:?}`, thread: `{}`",
                h.id, h.status, h.mode, h.thread_id
            ));
        }
        if let Some(sid) = session_id {
            lines.push(format!("Session: `{}`", sid));
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "kill" => {
            if rest.is_empty() || rest == "all" {
                let session_id = conn.read().await.subscriptions.first().cloned();
                if let Some(sid) = session_id {
                    match state
                        .agents
                        .acp
                        .terminate_session(&AcpSessionId(sid.clone()))
                        .await
                    {
                        Ok(count) => WsResponse::ok(
                            &req.id,
                            serde_json::json!({
                                "text": format!(
                                    "💀 Terminated {} subagent(s) in session `{}`.",
                                    count, sid
                                )
                            }),
                        ),
                        Err(e) => WsResponse::err(
                            &req.id,
                            "KILL_FAILED",
                            format!("Failed to terminate session: {}", e),
                        ),
                    }
                } else {
                    WsResponse::ok(
                        &req.id,
                        serde_json::json!({ "text": "💀 No active session to kill." }),
                    )
                }
            } else {
                match state.agents.acp.kill_subagent(rest).await {
                    Ok(true) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!("💀 Subagent `{}` killed.", rest)
                        }),
                    ),
                    Ok(false) => WsResponse::err(
                        &req.id,
                        "AGENT_NOT_FOUND",
                        format!("Subagent `{}` not found.", rest),
                    ),
                    Err(e) => WsResponse::err(
                        &req.id,
                        "KILL_FAILED",
                        format!("Failed to kill `{}`: {}", rest, e),
                    ),
                }
            }
        }
        "log" => {
            let topics = state.agents.acp.bus_topics().await;
            if topics.is_empty() {
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": "📜 No ACP bus topics." }),
                );
            }
            let mut lines = vec!["📜 **ACP Bus Log**".to_string()];
            for topic in topics {
                let subscribers = state.agents.acp.bus_subscribers(&topic).await;
                lines.push(format!(
                    "- `{}` — {} subscriber(s): {}",
                    topic,
                    subscribers.len(),
                    subscribers.join(", ")
                ));
            }
            WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
        }
        "info" => {
            if rest.is_empty() {
                return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /subagents info <id>");
            }
            let status = state.agents.acp.get_subagent_status(rest).await;
            let all = state.agents.acp.list_subagents().await;
            match all.iter().find(|h| h.id == rest) {
                Some(handle) => {
                    let text = format!(
                        "🤖 **Subagent `{}`**\n\nStatus: `{:?}`\nMode: `{:?}`\nThread: \
                         `{}`\nSession: `{}`\nParent: `{}`",
                        handle.id,
                        status.unwrap_or(handle.status),
                        handle.mode,
                        handle.thread_id,
                        handle.session_id,
                        handle.parent_id
                    );
                    WsResponse::ok(&req.id, serde_json::json!({ "text": text }))
                }
                None => WsResponse::err(
                    &req.id,
                    "AGENT_NOT_FOUND",
                    format!("Subagent `{}` not found.", rest),
                ),
            }
        }
        "send" | "steer" => {
            if rest.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /subagents send|steer <id> <message>",
                );
            }
            let msg_parts: Vec<&str> = rest.splitn(2, ' ').collect();
            let target_id = msg_parts[0];
            let message = msg_parts.get(1).unwrap_or(&"").trim();
            if message.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /subagents send|steer <id> <message>",
                );
            }

            let guard = conn.read().await;
            let sender = guard
                .user_id
                .as_ref()
                .map(|u| u.0.clone())
                .unwrap_or_else(|| "user".to_string());
            let conversation_id = guard.subscriptions.first().cloned().unwrap_or_default();
            drop(guard);

            let incoming =
                crate::channels::IncomingMessage::new(sender, conversation_id, message.to_string());

            if sub == "send" {
                match state.agents.acp.send_message(target_id, incoming).await {
                    Ok(result) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!(
                                "🤖 Message sent to `{}`.\n\nResponse: {}",
                                target_id, result
                            )
                        }),
                    ),
                    Err(e) => WsResponse::err(
                        &req.id,
                        "SEND_FAILED",
                        format!("Failed to send to `{}`: {}", target_id, e),
                    ),
                }
            } else {
                match state
                    .agents
                    .acp
                    .steer_subagent(target_id, message.to_string())
                    .await
                {
                    Ok(result) => WsResponse::ok(
                        &req.id,
                        serde_json::json!({
                            "text": format!(
                                "🤖 Steering sent to `{}`.\n\nResult: {}",
                                target_id, result
                            )
                        }),
                    ),
                    Err(e) => WsResponse::err(
                        &req.id,
                        "STEER_FAILED",
                        format!("Failed to steer `{}`: {}", target_id, e),
                    ),
                }
            }
        }
        "spawn" => {
            let session_id = conn.read().await.subscriptions.first().cloned();
            let Some(sid) = session_id else {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /subagents spawn requires an active session.",
                );
            };
            let route = state.agents.router.resolve_by_session(&sid).await;
            if route.agent_id.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "NO_PARENT_AGENT",
                    "No parent agent found for the active session.",
                );
            }

            let (_agent_type, system_prompt) = if rest.is_empty() {
                ("default".to_string(), None)
            } else {
                let mut words = rest.splitn(2, ' ');
                let at = words.next().unwrap_or("default").to_string();
                let prompt = words.next().map(|s| s.to_string());
                (at, prompt)
            };

            let config = SubagentConfig {
                system_prompt,
                mode: SpawnMode::Run,
                thread_binding: ThreadBinding::Auto,
                ..SubagentConfig::default()
            };

            match state
                .agents
                .acp
                .spawn_subagent(AcpSessionId(sid.clone()), route.agent_id, config)
                .await
            {
                Ok(handle) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "text": format!(
                            "🤖 Spawned subagent `{}` in session `{}`.",
                            handle.id, sid
                        )
                    }),
                ),
                Err(e) => WsResponse::err(
                    &req.id,
                    "SPAWN_FAILED",
                    format!("Failed to spawn subagent: {}", e),
                ),
            }
        }
        _ => WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /subagents list|kill|log|info|send|steer|spawn",
        ),
    }
}

pub(super) async fn handle_acp(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        let session_id = conn.read().await.subscriptions.first().cloned();
        if let Some(sid) = session_id {
            if let Ok(Some(status)) = state.agents.acp.get_status(sid.clone()).await {
                let text = format!(
                    "🤖 **ACP Session `{}`**\n\nState: `{:?}`\nMode: `{:?}`\nIteration: \
                     {}/{}\nQueue depth: {}",
                    sid,
                    status.runtime_state,
                    status.mode,
                    status.current_iteration,
                    status.max_iterations,
                    status.queue_depth,
                );
                return WsResponse::ok(&req.id, serde_json::json!({ "text": text }));
            }
        }
        return WsResponse::ok(&req.id, serde_json::json!({ "text": "🤖 No active ACP session." }));
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let sub = parts[0];
    let rest = parts.get(1).unwrap_or(&"").trim();

    match sub {
        "spawn" => {
            let parent_id = if rest.is_empty() {
                let session_id = conn.read().await.subscriptions.first().cloned();
                if let Some(sid) = session_id {
                    let route = state.agents.router.resolve_by_session(&sid).await;
                    if route.agent_id.is_empty() {
                        None
                    } else {
                        Some(route.agent_id)
                    }
                } else {
                    None
                }
            } else {
                Some(rest.to_string())
            };
            let Some(parent_id) = parent_id else {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /acp spawn [parent_agent_id] (requires an active session or explicit \
                     parent)",
                );
            };
            let session_id = state.agents.acp.create_session(parent_id.clone()).await;
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "text": format!(
                        "🤖 Created ACP session `{}` for parent `{}`.",
                        session_id, parent_id
                    )
                }),
            )
        }
        "cancel" => {
            let sid = if rest.is_empty() {
                conn.read().await.subscriptions.first().cloned()
            } else {
                Some(rest.to_string())
            };
            if let Some(sid) = sid {
                if let Err(e) = state.agents.acp.cancel(sid.clone()).await {
                    warn!("Failed to cancel ACP session {}: {}", sid, e);
                }
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🤖 ACP session `{}` cancelled.", sid) }),
                );
            }
            WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /acp cancel [session_id]")
        }
        "close" => {
            let sid = if rest.is_empty() {
                conn.read().await.subscriptions.first().cloned()
            } else {
                Some(rest.to_string())
            };
            if let Some(sid) = sid {
                if let Err(e) = state
                    .agents
                    .acp
                    .terminate_session(&AcpSessionId(sid.clone()))
                    .await
                {
                    warn!("Failed to terminate ACP session {}: {}", sid, e);
                }
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": format!("🤖 ACP session `{}` terminated.", sid) }),
                );
            }
            WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /acp close [session_id]")
        }
        "steer" => {
            if rest.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /acp steer <id> <message>",
                );
            }
            let steer_parts: Vec<&str> = rest.splitn(2, ' ').collect();
            let target_id = steer_parts[0];
            let message = steer_parts.get(1).unwrap_or(&"").trim();
            if message.is_empty() {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    "Usage: /acp steer <id> <message>",
                );
            }
            match state
                .agents
                .acp
                .steer_subagent(target_id, message.to_string())
                .await
            {
                Ok(result) => WsResponse::ok(
                    &req.id,
                    serde_json::json!({
                        "text": format!(
                            "🤖 Steering sent to `{}`.\n\nResult: {}",
                            target_id, result
                        )
                    }),
                ),
                Err(e) => WsResponse::err(
                    &req.id,
                    "STEER_FAILED",
                    format!("Failed to steer `{}`: {}", target_id, e),
                ),
            }
        }
        "sessions" => {
            let subagents = state.agents.acp.list_subagents().await;
            let mut session_ids: Vec<AcpSessionId> =
                subagents.iter().map(|h| h.session_id.clone()).collect();
            session_ids.sort_by(|a, b| a.0.cmp(&b.0));
            session_ids.dedup();

            if session_ids.is_empty() {
                return WsResponse::ok(
                    &req.id,
                    serde_json::json!({ "text": "🤖 No ACP sessions." }),
                );
            }

            let mut lines = vec![format!("🤖 **ACP Sessions** ({})", session_ids.len())];
            for sid in session_ids {
                match state.agents.acp.get_session_info(&sid).await {
                    Some(info) => lines.push(format!(
                        "- `{}` — parent `{}`, {} subagent(s), created {}",
                        info.id, info.parent_agent_id, info.subagent_count, info.created_at
                    )),
                    None => lines.push(format!("- `{}` — metadata unavailable", sid)),
                }
            }
            WsResponse::ok(&req.id, serde_json::json!({ "text": lines.join("\n") }))
        }
        "pause" | "resume" | "step" => {
            let sid = if rest.is_empty() {
                conn.read().await.subscriptions.first().cloned()
            } else {
                Some(rest.to_string())
            };
            let Some(sid) = sid else {
                return WsResponse::err(
                    &req.id,
                    "INVALID_ARGS",
                    format!("Usage: /acp {} [session_id]", sub),
                );
            };
            match sub {
                "pause" => {
                    if let Err(e) = state.agents.acp.pause(sid.clone()).await {
                        warn!("Failed to pause ACP session {}: {}", sid, e);
                    }
                }
                "resume" => {
                    if let Err(e) = state.agents.acp.resume(sid.clone()).await {
                        warn!("Failed to resume ACP session {}: {}", sid, e);
                    }
                }
                "step" => {
                    if let Err(e) = state.agents.acp.step(sid.clone()).await {
                        warn!("Failed to step ACP session {}: {}", sid, e);
                    }
                }
                _ => {
                    return WsResponse::err(
                        &req.id,
                        "INVALID_ARGS",
                        format!("Unknown ACP subcommand: {}", sub),
                    );
                }
            }
            WsResponse::ok(
                &req.id,
                serde_json::json!({
                    "text": format!("🤖 Sent `{}` to session `{}`.", sub, sid)
                }),
            )
        }
        _ => WsResponse::err(
            &req.id,
            "INVALID_ARGS",
            "Usage: /acp spawn|cancel|steer|close|sessions|status|pause|resume|step",
        ),
    }
}

pub(super) async fn handle_steer(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /steer <id> <message>");
    }

    let parts: Vec<&str> = trimmed.splitn(2, ' ').collect();
    let target_id = parts[0];
    let message = parts.get(1).unwrap_or(&"").trim();

    if message.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /steer <id> <message>");
    }

    let incoming = crate::channels::IncomingMessage::new(
        conn.read()
            .await
            .user_id
            .as_ref()
            .map(|u| u.0.clone())
            .unwrap_or_else(|| "user".to_string()),
        conn.read()
            .await
            .subscriptions
            .first()
            .cloned()
            .unwrap_or_default(),
        message.to_string(),
    );

    match state.agents.acp.send_message(target_id, incoming).await {
        Ok(result) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🤖 Steering sent to `{}`. Result: {}", target_id, result) }),
        ),
        Err(e) => WsResponse::err(
            &req.id,
            "STEER_FAILED",
            format!("Failed to steer `{}`: {}", target_id, e),
        ),
    }
}

pub(super) async fn handle_kill(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "all" {
        let session_id = conn.read().await.subscriptions.first().cloned();
        if let Some(sid) = session_id {
            if let Err(e) = state.agents.acp.cancel(sid.clone()).await {
                warn!("Failed to send kill signal to session {}: {}", sid, e);
            }
            return WsResponse::ok(
                &req.id,
                serde_json::json!({ "text": format!("💀 Kill signal sent to session `{}`.", sid) }),
            );
        }
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": "💀 No active session to kill." }),
        );
    }

    // Try to shutdown the specific subagent
    match state.agents.acp.shutdown_subagent(trimmed).await {
        Ok(true) => WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("💀 Subagent `{}` shutdown initiated.", trimmed) }),
        ),
        Ok(false) => WsResponse::err(
            &req.id,
            "AGENT_NOT_FOUND",
            format!("Subagent `{}` not found.", trimmed),
        ),
        Err(e) => {
            WsResponse::err(&req.id, "KILL_FAILED", format!("Failed to kill `{}`: {}", trimmed, e))
        }
    }
}

pub(super) async fn handle_focus(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    args: &str,
) -> WsResponse {
    let target = args.trim();
    if target.is_empty() {
        return WsResponse::err(&req.id, "INVALID_ARGS", "Usage: /focus <target>");
    }

    let session_id = conn.read().await.subscriptions.first().cloned();
    if let Some(sid) = session_id {
        let result = crate::inbound::router::RouteResult {
            agent_id: target.to_string(),
            workspace_id: None,
            persisted_binding: true,
            is_fallback: false,
        };
        state.agents.router.bind_session(&sid, &result).await;
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🎯 Session `{}` bound to agent '{}'.", sid, target) }),
        );
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "🎯 No active session to focus." }))
}

pub(super) async fn handle_unfocus(
    req: &WsRequest,
    conn: &Arc<RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let session_id = conn.read().await.subscriptions.first().cloned();
    if let Some(sid) = session_id {
        state.agents.router.unbind_session(&sid).await;
        return WsResponse::ok(
            &req.id,
            serde_json::json!({ "text": format!("🎯 Session `{}` unbound.", sid) }),
        );
    }

    WsResponse::ok(&req.id, serde_json::json!({ "text": "🎯 No active session to unfocus." }))
}
