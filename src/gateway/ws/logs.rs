//! logs.subscribe / logs.unsubscribe (log tail) handlers.

use super::*;
pub(super) async fn handle_logs_subscribe(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
    cmd_tx: &mpsc::Sender<WsCommand>,
) -> WsResponse {
    // Cancel any existing log subscription for this connection and remove its
    // task from the registry so we don't leak aborted tasks.
    let (conn_id, prev_cancel_tx) = {
        let cg = conn.write().await;
        let conn_id = cg.conn_id.clone();
        let prev_cancel_tx = cg.log_cancel_tx.clone();
        (conn_id, prev_cancel_tx)
    };
    if let Some(tx) = prev_cancel_tx {
        if let Err(e) = tx.send(()).await {
            warn!("Failed to cancel previous log tail for {}: {}", conn_id, e);
        }
    }
    state
        .task_registry
        .abort(&format!("ws:log_tail:{}", conn_id))
        .await;

    let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
    {
        let mut cg = conn.write().await;
        cg.log_cancel_tx = Some(cancel_tx);
    }

    let log_tx = state.events.log_tx.clone();
    let cmd_tx = cmd_tx.clone();
    let task_registry = state.task_registry.clone();
    let shutdown_token = state.shutdown_token.clone();
    let conn_id_for_task = conn_id.clone();

    let task_handle = tokio::spawn(async move {
        // Subscribe to new log lines first to avoid missing any during file read
        let mut log_rx = log_tx.subscribe();

        // Send all historical log lines from the file
        let log_path = crate::logs::log_file_path();
        if log_path.exists() {
            if let Ok(file) = tokio::fs::File::open(&log_path).await {
                let reader = tokio::io::BufReader::new(file);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let event = WsEvent {
                        frame_type: "event",
                        event: "log.line".to_string(),
                        payload: serde_json::to_value(serde_json::json!({
                            "line": line,
                            "historical": true,
                        }))
                        .ok(),
                        seq: None,
                    };
                    if let Ok(text) = serde_json::to_string(&event) {
                        if cmd_tx.send(WsCommand::SendEvent(text)).await.is_err() {
                            warn!("Log tail send channel closed for {}", conn_id_for_task);
                            break;
                        }
                    }
                }
            }
        }

        // Forward new lines from the broadcast channel
        loop {
            tokio::select! {
                Ok(line) = log_rx.recv() => {
                    let event = WsEvent {
                        frame_type: "event",
                        event: "log.line".to_string(),
                        payload: serde_json::to_value(serde_json::json!({
                            "line": line,
                            "historical": false,
                        })).ok(),
                        seq: None,
                    };
                    if let Ok(text) = serde_json::to_string(&event) {
                        if cmd_tx
                            .send(WsCommand::SendEvent(text))
                            .await
                            .is_err()
                        {
                            warn!("Log tail send channel closed for {}", conn_id_for_task);
                            break;
                        }
                    }
                }
                _ = cancel_rx.recv() => {
                    break;
                }
                _ = shutdown_token.cancelled() => {
                    info!("Log tail task received shutdown signal for {}", conn_id_for_task);
                    break;
                }
            }
        }
    });

    task_registry
        .insert_join(format!("ws:log_tail:{}", conn_id), task_handle)
        .await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "subscribed" }))
}

pub(super) async fn handle_logs_unsubscribe(
    req: &WsRequest,
    conn: &Arc<tokio::sync::RwLock<ProtocolConnection>>,
    state: &Arc<GatewayState>,
) -> WsResponse {
    let (conn_id, cancel_tx) = {
        let mut cg = conn.write().await;
        let conn_id = cg.conn_id.clone();
        let cancel_tx = cg.log_cancel_tx.take();
        (conn_id, cancel_tx)
    };
    if let Some(tx) = cancel_tx {
        if let Err(e) = tx.send(()).await {
            warn!("Failed to cancel log tail for {}: {}", conn_id, e);
        }
    }
    state
        .task_registry
        .abort(&format!("ws:log_tail:{}", conn_id))
        .await;

    WsResponse::ok(&req.id, serde_json::json!({ "status": "unsubscribed" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::{make_test_conn, make_test_state};
    use crate::gateway::GatewayConfig;

    fn req(id: &str) -> WsRequest {
        WsRequest {
            frame_type: "req".into(),
            id: id.into(),
            method: "x".into(),
            params: None,
        }
    }

    async fn state() -> Arc<GatewayState> {
        Arc::new(make_test_state(GatewayConfig::default()).await)
    }

    #[tokio::test]
    async fn logs_subscribe_then_unsubscribe_ok() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<WsCommand>(1);

        let resp = handle_logs_subscribe(&req("r1"), &conn, &state, &cmd_tx).await;
        assert!(resp.ok);
        assert_eq!(resp.payload.as_ref().unwrap()["status"], "subscribed");

        // A cancel sender is now stored on the connection.
        {
            let cg = conn.read().await;
            assert!(cg.log_cancel_tx.is_some());
        }

        let resp = handle_logs_unsubscribe(&req("r2"), &conn, &state).await;
        assert!(resp.ok);
        assert_eq!(resp.payload.as_ref().unwrap()["status"], "unsubscribed");

        // Cancel sender taken (cleared) after unsubscribe.
        {
            let cg = conn.read().await;
            assert!(cg.log_cancel_tx.is_none());
        }
        // Ensure the tail task is gone from the registry.
        state.task_registry.abort("ws:log_tail:test-conn").await;
    }

    #[tokio::test]
    async fn logs_unsubscribe_without_subscription_ok() {
        let state = state().await;
        let conn = make_test_conn(&[]);
        let resp = handle_logs_unsubscribe(&req("r1"), &conn, &state).await;
        assert!(resp.ok);
        assert_eq!(resp.payload.as_ref().unwrap()["status"], "unsubscribed");
    }
}
