//! Shared goal-runner lifecycle helpers.
//!
//! A goal is *suspended* when it has a persisted checkpoint but no live
//! runner — the state after a gateway restart (startup deliberately does not
//! re-arm autonomous loops) or after a policy stop (loop detected / max
//! rounds, which keep their checkpoint). `spawn_goal_runner` is the single
//! place that turns a persisted checkpoint back into a running goal; today
//! its only caller is `/goal resume`.

use std::sync::Arc;

use tracing::warn;

use super::GatewayState;
use crate::goal::persist::PersistedGoalState;

/// A persisted goal with no live runner.
#[derive(Debug)]
pub(crate) struct SuspendedGoal {
    pub goal_id: String,
    pub round: usize,
    pub max_rounds: usize,
    pub blocked_reason: Option<crate::goal::BlockedReason>,
}

/// List goals that are persisted but not running (suspended).
pub(crate) async fn list_suspended(state: &Arc<GatewayState>) -> Vec<SuspendedGoal> {
    let store = crate::goal::persist::GoalStore::new();
    let persisted = store.load_all().await;
    let cancellers = state.agents.goal_cancellers.read().await;
    persisted
        .iter()
        .filter(|p| !cancellers.contains_key(&p.goal_id))
        .map(|p| SuspendedGoal {
            goal_id: p.goal_id.clone(),
            round: p.round,
            max_rounds: p.plan.max_rounds,
            blocked_reason: p.blocked_reason.clone(),
        })
        .collect()
}

/// Spawn a goal runner from a persisted checkpoint: event relay, canceller
/// registration, and the runner task. Returns the goal id.
pub(crate) async fn spawn_goal_runner(
    state: &Arc<GatewayState>,
    persisted: &PersistedGoalState,
) -> String {
    let goal_id = persisted.goal_id.clone();
    let (goal_tx, mut goal_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_tx = state.events.tx.clone();
    let gid = goal_id.clone();
    let s_for_relay = persisted.parent_session_id.clone();

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

    let (goal_id, parent_sid, plan, condition_history) =
        crate::goal::persist::to_runner_params(persisted);

    let runner = crate::goal::GoalRunner::new(
        &goal_id,
        &parent_sid,
        plan,
        state.tools.registry.clone(),
        state.infra.model_router.clone(),
        goal_tx,
    )
    .with_store(crate::goal::persist::shared_store())
    .with_progress(persisted.round, condition_history)
    // Fresh-context goals resume with the same carried handoff they had
    // in-process — a restart must not silently drop the only inter-round
    // state.
    .with_handoff(persisted.last_handoff.clone());

    let cancel_token = runner.cancel_token();
    {
        let mut cancellers = state.agents.goal_cancellers.write().await;
        cancellers.insert(goal_id.clone(), cancel_token);
    }

    let gid2 = goal_id.clone();
    let cancellers = state.agents.goal_cancellers.clone();
    tokio::spawn(async move {
        runner.run().await;
        let mut c = cancellers.write().await;
        c.remove(&gid2);
    });

    goal_id
}
