use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::acp::config::{
    CrashRecoveryConfig, SpawnMode, SubagentConfig, SubagentStatus, ThreadBinding,
};
use crate::acp::subagent::SubagentCommand;
use crate::agent::{Agent, AgentConfig};
use crate::channels::IncomingMessage;

fn mock_agent_builder() -> impl Fn(&str) -> crate::Result<Agent> + Send + Sync + 'static {
    |_subagent_id| {
        let provider = Arc::new(
            crate::providers::mock::MockProvider::new()
                .with_responses(vec![crate::providers::Message::assistant("mock response")]),
        );
        let tools = Arc::new(crate::tools::ToolRegistry::new());
        let config = AgentConfig::default();
        Ok(Agent::new(config, provider, tools))
    }
}

/// Spawn a subagent whose task panics and verify it is automatically
/// recovered.
#[tokio::test]
async fn test_subagent_crash_auto_recovery() {
    let crashed = Arc::new(AtomicBool::new(false));
    let crashed_for_builder = crashed.clone();
    let acp = AcpControlPlane::new(50)
        .with_recovery(CrashRecoveryConfig {
            enabled: true,
            max_retries: 1,
            backoff_seconds: vec![0],
        })
        .with_agent_builder(move |_subagent_id| {
            let crashed = crashed_for_builder.clone();
            let provider = Arc::new(crate::providers::mock::MockProvider::new().with_callback(
                move |_messages| {
                    if !crashed.swap(true, Ordering::SeqCst) {
                        panic!("simulated subagent crash")
                    }
                    crate::providers::Message::assistant("recovered")
                },
            ));
            let tools = Arc::new(crate::tools::ToolRegistry::new());
            let config = AgentConfig::default();
            Ok(Agent::new(config, provider, tools))
        });

    let session_id = acp.create_session("parent".to_string()).await;
    let config = SubagentConfig {
        mode: SpawnMode::Run,
        ..SubagentConfig::default()
    };

    let handle = acp
        .spawn_subagent(session_id.clone(), "parent".to_string(), config)
        .await
        .expect("spawn subagent");

    // Send a message to trigger processing (and the simulated panic).
    let msg = IncomingMessage::new(
        "user".to_string(),
        format!("conv-{}", handle.id),
        "trigger".to_string(),
    );
    let _ = acp.send_message(&handle.id, msg).await.ok();

    // Poll until recovery completes or a timeout is reached.
    let recovered = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let subs = acp.list_session_subagents(&session_id).await;
            if let Some(candidate) = subs.first() {
                if candidate.id != handle.id
                    && candidate.crash_count == 1
                    && candidate.status == SubagentStatus::Ready
                {
                    return candidate.clone();
                }
            }
        }
    })
    .await
    .expect("recovery should complete within 5 seconds");

    // The original handle is replaced during recovery; it may no longer be
    // present in the subagent map.
    let original_status = acp.get_subagent_status(&handle.id).await;
    assert!(
        original_status.is_none() || original_status == Some(SubagentStatus::Crashed),
        "original handle should be removed or marked Crashed"
    );

    assert_eq!(recovered.status, SubagentStatus::Ready);
    assert_eq!(recovered.crash_count, 1);

    // Cleanup
    acp.shutdown_subagent(&recovered.id)
        .await
        .expect("shutdown recovered subagent");
}

#[tokio::test]
async fn test_thread_context_switch_and_migration() {
    let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
    let session_id = acp.create_session("parent".to_string()).await;

    let s1 = acp
        .spawn_subagent(
            session_id.clone(),
            "parent".to_string(),
            SubagentConfig {
                thread_binding: ThreadBinding::Thread("thread-a".to_string()),
                ..SubagentConfig::default()
            },
        )
        .await
        .expect("spawn s1");

    let s2 = acp
        .spawn_subagent(
            session_id.clone(),
            "parent".to_string(),
            SubagentConfig {
                thread_binding: ThreadBinding::Thread("thread-a".to_string()),
                ..SubagentConfig::default()
            },
        )
        .await
        .expect("spawn s2");

    // Context switch: make s1 the active subagent on thread-a.
    acp.switch_thread_active_subagent("thread-a", Some(&s1.id))
        .await
        .expect("switch to s1");
    let ctx_a = acp
        .get_thread_context("thread-a")
        .await
        .expect("thread-a exists");
    assert_eq!(ctx_a.active_subagent, Some(s1.id.clone()));

    // Migrate s1 to thread-b.
    acp.migrate_subagent_thread(&s1.id, "thread-b")
        .await
        .expect("migrate to thread-b");

    // s1 should now be bound to thread-b.
    let session_subagents = acp.list_session_subagents(&session_id).await;
    let s1_after = session_subagents
        .iter()
        .find(|h| h.id == s1.id)
        .expect("s1 still registered");
    assert_eq!(s1_after.thread_id, "thread-b");

    // thread-a should have cleared its active subagent.
    let ctx_a = acp
        .get_thread_context("thread-a")
        .await
        .expect("thread-a exists");
    assert!(ctx_a.active_subagent.is_none());

    // thread-b should have s1 as active subagent.
    let ctx_b = acp
        .get_thread_context("thread-b")
        .await
        .expect("thread-b exists");
    assert_eq!(ctx_b.active_subagent, Some(s1.id.clone()));

    // s2 should remain on thread-a.
    let s2_after = session_subagents
        .iter()
        .find(|h| h.id == s2.id)
        .expect("s2 still registered");
    assert_eq!(s2_after.thread_id, "thread-a");

    // Context switch s2 to active on thread-a.
    acp.switch_thread_active_subagent("thread-a", Some(&s2.id))
        .await
        .expect("switch to s2");
    let ctx_a = acp
        .get_thread_context("thread-a")
        .await
        .expect("thread-a exists");
    assert_eq!(ctx_a.active_subagent, Some(s2.id.clone()));

    // Cleanup
    acp.shutdown_subagent(&s1.id).await.expect("shutdown s1");
    acp.shutdown_subagent(&s2.id).await.expect("shutdown s2");
}

#[tokio::test]
async fn test_cross_session_subagent_bus() {
    let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
    let session_a = acp.create_session("parent-a".to_string()).await;
    let session_b = acp.create_session("parent-b".to_string()).await;

    let s1 = acp
        .spawn_subagent(session_a, "parent-a".to_string(), SubagentConfig::default())
        .await
        .expect("spawn s1");
    let s2 = acp
        .spawn_subagent(session_b, "parent-b".to_string(), SubagentConfig::default())
        .await
        .expect("spawn s2");

    // Subscribe s2 to the shared topic; s1 will publish without subscribing.
    acp.bus_subscribe(&s2.id, "alerts")
        .await
        .expect("subscribe s2");

    // Publish from s1 in session A.
    let msg = acp
        .bus_publish(&s1.id, "alerts", "hello from session A")
        .await
        .expect("publish");
    assert_eq!(msg.sender_id, s1.id);
    assert_eq!(msg.payload, "hello from session A");

    // s2 in session B receives the message.
    let pending = acp.bus_poll(&s2.id, "alerts").await.expect("poll s2");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].payload, "hello from session A");

    // A second poll returns nothing new.
    let pending_again = acp.bus_poll(&s2.id, "alerts").await.expect("poll s2 again");
    assert!(pending_again.is_empty());

    // Topic and subscriber introspection.
    let topics = acp.bus_topics().await;
    assert!(topics.contains(&"alerts".to_string()));

    let subscribers = acp.bus_subscribers("alerts").await;
    assert_eq!(subscribers, vec![s2.id.clone()]);

    // Unsubscribe s2 and confirm it no longer receives messages.
    acp.bus_unsubscribe(&s2.id, "alerts").await;
    acp.bus_publish(&s1.id, "alerts", "after unsubscribe")
        .await
        .expect("publish after unsubscribe");
    let after_unsub = acp
        .bus_poll(&s2.id, "alerts")
        .await
        .expect("poll after unsub");
    assert!(after_unsub.is_empty());

    // Cleanup
    acp.shutdown_subagent(&s1.id).await.expect("shutdown s1");
    acp.shutdown_subagent(&s2.id).await.expect("shutdown s2");
}

#[tokio::test]
async fn test_acp_control_plane_new() {
    let acp = AcpControlPlane::new(50);
    let subagents = acp.list_subagents().await;
    assert!(subagents.is_empty());
}

#[tokio::test]
async fn test_create_session() {
    let acp = AcpControlPlane::new(50);
    let session_id = acp.create_session("parent-1".to_string()).await;
    assert!(!session_id.0.is_empty());

    let info = acp.get_session_info(&session_id).await;
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.parent_agent_id, "parent-1");
    assert_eq!(info.subagent_count, 0);
}

#[tokio::test]
async fn test_get_session_info_not_found() {
    let acp = AcpControlPlane::new(50);
    let info = acp
        .get_session_info(&AcpSessionId("nonexistent".to_string()))
        .await;
    assert!(info.is_none());
}

#[tokio::test]
async fn test_terminate_session_not_found() {
    let acp = AcpControlPlane::new(50);
    let result = acp
        .terminate_session(&AcpSessionId("nonexistent".to_string()))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_session_subagents_empty() {
    let acp = AcpControlPlane::new(50);
    let session_id = acp.create_session("parent".to_string()).await;
    let subagents = acp.list_session_subagents(&session_id).await;
    assert!(subagents.is_empty());
}

#[tokio::test]
async fn test_get_subagent_status_not_found() {
    let acp = AcpControlPlane::new(50);
    let status = acp.get_subagent_status("nonexistent").await;
    assert!(status.is_none());
}

#[tokio::test]
async fn test_concurrent_subagent_spawn() {
    let acp = AcpControlPlane::new(50);
    acp.set_agent_builder(mock_agent_builder()).await;
    let session_id = acp.create_session("parent-1".to_string()).await;

    let mut spawn_tasks = Vec::new();
    for i in 0..10usize {
        let acp_clone = acp.clone();
        let sid = session_id.clone();
        let config = SubagentConfig {
            mode: SpawnMode::Run,
            thread_binding: ThreadBinding::Auto,
            system_prompt: Some(format!("subagent-{}", i)),
            timeout_seconds: Some(30),
        };
        spawn_tasks.push(tokio::spawn(async move {
            acp_clone
                .spawn_subagent(sid, "parent-1".to_string(), config)
                .await
        }));
    }

    let results = futures::future::join_all(spawn_tasks).await;
    let mut handles = Vec::new();
    for result in results {
        let handle = result
            .expect("spawn task should not panic")
            .expect("spawn_subagent should succeed");
        assert!(
            handle
                .command_tx
                .send(SubagentCommand::Shutdown)
                .await
                .is_ok(),
            "subagent should accept shutdown"
        );
        handles.push(handle);
    }

    // All 10 subagents should have been created with unique IDs
    assert_eq!(handles.len(), 10);
    let ids: std::collections::HashSet<_> = handles.iter().map(|h| h.id.clone()).collect();
    assert_eq!(ids.len(), 10, "all subagent IDs should be unique");
}

#[tokio::test]
async fn test_acp_lifecycle_events_are_emitted() {
    let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);
    acp.set_event_tx(event_tx).await;

    let session_id = acp.create_session("parent".to_string()).await;
    let handle = acp
        .spawn_subagent(
            session_id.clone(),
            "parent".to_string(),
            SubagentConfig {
                mode: SpawnMode::Run,
                thread_binding: ThreadBinding::New,
                ..SubagentConfig::default()
            },
        )
        .await
        .expect("spawn subagent");

    let event = event_rx.recv().await.expect("receive spawned event");
    match event {
        crate::gateway::GatewayEvent::AcpSpawned {
            session_id: sid,
            subagent_id,
            parent_id,
            mode,
            ..
        } => {
            assert_eq!(sid, session_id.to_string());
            assert_eq!(subagent_id, handle.id);
            assert_eq!(parent_id, "parent");
            assert_eq!(mode, "run");
        }
        other => panic!("expected AcpSpawned event, got {:?}", other),
    }

    let _ = handle
        .command_tx
        .send(SubagentCommand::Shutdown)
        .await
        .expect("send shutdown to subagent");
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let completed = event_rx.recv().await.expect("receive completed event");
    match completed {
        crate::gateway::GatewayEvent::AcpCompleted { subagent_id, status, .. } => {
            assert_eq!(subagent_id, handle.id);
            assert_eq!(status, "terminated");
        }
        other => panic!("expected AcpCompleted event, got {:?}", other),
    }
}

#[tokio::test]
async fn test_acp_control_plane_has_store_without_store() {
    let acp = AcpControlPlane::new(50);
    assert!(!acp.has_store());
}

#[tokio::test]
async fn test_pause_resume_step_cancel_emit_status_changed_after_actor_processing() {
    let acp = AcpControlPlane::new(50).with_agent_builder(mock_agent_builder());
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(16);
    acp.set_event_tx(event_tx).await;

    // Create a session actor by executing a message.
    let agent = mock_agent_builder()("test-subagent").expect("mock agent builds");
    let msg = IncomingMessage::new("user1", "conv1", "hello");
    let _ = acp.execute_session(Arc::new(agent), msg).await.ok();

    // Pause: event should report the actual state after the actor processed it.
    acp.pause("conv1".to_string())
        .await
        .expect("pause command sent");
    let event = event_rx.recv().await.expect("receive pause event");
    assert!(
        matches!(
            event,
            crate::gateway::GatewayEvent::AcpStatusChanged {
                ref session_id,
                ref runtime_state,
            } if session_id == "conv1" && runtime_state == "paused"
        ),
        "expected AcpStatusChanged(paused), got {:?}",
        event
    );

    // Resume.
    acp.resume("conv1".to_string())
        .await
        .expect("resume command sent");
    let event = event_rx.recv().await.expect("receive resume event");
    assert!(
        matches!(
            event,
            crate::gateway::GatewayEvent::AcpStatusChanged {
                ref session_id,
                ref runtime_state,
            } if session_id == "conv1" && runtime_state == "running"
        ),
        "expected AcpStatusChanged(running), got {:?}",
        event
    );

    // Step.
    acp.step("conv1".to_string())
        .await
        .expect("step command sent");
    let event = event_rx.recv().await.expect("receive step event");
    assert!(
        matches!(
            event,
            crate::gateway::GatewayEvent::AcpStatusChanged {
                ref session_id,
                ref runtime_state,
            } if session_id == "conv1" && runtime_state == "stepping"
        ),
        "expected AcpStatusChanged(stepping), got {:?}",
        event
    );

    // Cancel.
    acp.cancel("conv1".to_string())
        .await
        .expect("cancel command sent");
    let event = event_rx.recv().await.expect("receive cancel event");
    assert!(
        matches!(
            event,
            crate::gateway::GatewayEvent::AcpStatusChanged {
                ref session_id,
                ref runtime_state,
            } if session_id == "conv1" && runtime_state == "cancelled"
        ),
        "expected AcpStatusChanged(cancelled), got {:?}",
        event
    );
}

#[tokio::test]
async fn test_agent_builder_receives_subagent_id() {
    use std::sync::Mutex;

    let received: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let received_for_builder = Arc::clone(&received);
    let acp = AcpControlPlane::new(50).with_agent_builder(move |subagent_id| {
        *received_for_builder.lock().unwrap() = Some(subagent_id.to_string());
        let provider = Arc::new(
            crate::providers::mock::MockProvider::new()
                .with_responses(vec![crate::providers::Message::assistant("mock response")]),
        );
        let tools = Arc::new(crate::tools::ToolRegistry::new());
        let config = AgentConfig {
            agent_id: Some(subagent_id.to_string()),
            ..AgentConfig::default()
        };
        Ok(Agent::new(config, provider, tools))
    });

    let session_id = acp.create_session("parent".to_string()).await;
    let handle = acp
        .spawn_subagent(session_id, "parent".to_string(), SubagentConfig::default())
        .await
        .expect("subagent spawns");
    assert_eq!(
        received.lock().unwrap().as_deref(),
        Some(handle.id.as_str()),
        "agent builder must receive the subagent id so turn records are tagged"
    );
}
