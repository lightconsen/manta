//! WS `feedback.vote` — per-turn Like/Dislike feedback.
//!
//! The client sends the stable `turn_id` received in the `chat.final` event.
//! An `up`/`down` vote is upserted into `turn_feedback`; a `down` vote also
//! queues a `human:dislike` pending badcase (deduped on input||response).

use super::*;
use crate::eval::{InsertPendingParams, PendingSource};
use crate::gateway::FeedbackVoteKind;

/// `feedback.vote` request params.
#[derive(Debug, Deserialize)]
pub struct FeedbackVoteParams {
    /// Stable per-turn id from the `chat.final` event.
    pub turn_id: String,
    /// "up" or "down".
    pub vote: String,
    /// Optional free-text comment.
    pub comment: Option<String>,
    /// Optional user input for the turn (used to seed a `human:dislike`
    /// pending badcase; not required to record the vote itself).
    pub input: Option<String>,
    /// Optional assistant response for the turn (paired with `input`).
    pub response: Option<String>,
}

pub(super) async fn handle_feedback_vote(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let params: FeedbackVoteParams = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    if params.turn_id.trim().is_empty() {
        return error_invalid_request(&req.id, "turn_id must be non-empty");
    }
    let vote = match FeedbackVoteKind::from_str(params.vote.trim()) {
        Some(v) => v,
        None => {
            return error_invalid_request(
                &req.id,
                format!("vote must be \"up\" or \"down\", got \"{}\"", params.vote),
            )
        }
    };

    let Some(store) = state.infra.feedback_store.as_ref() else {
        return WsResponse::err(
            &req.id,
            "FEEDBACK_UNAVAILABLE",
            "feedback store is not initialized (SQLite storage required)",
        );
    };

    if let Err(e) = store
        .upsert_vote(&crate::gateway::UpsertVoteParams {
            turn_id: params.turn_id.clone(),
            session_id: None,
            agent_id: None,
            vote,
            comment: params.comment.clone(),
        })
        .await
    {
        warn!("feedback.vote upsert failed for turn {}: {}", params.turn_id, e);
        return error_internal(&req.id, "failed to persist feedback");
    }

    // A 👎 vote seeds a pending badcase for the human-review loop, deduped on
    // input||response. Skip silently when the client did not supply both texts.
    if vote == FeedbackVoteKind::Down {
        if let (Some(input), Some(response)) = (params.input.clone(), params.response.clone()) {
            if let Some(pending) = state.infra.pending_badcase_store.as_ref() {
                match pending
                    .insert_pending(&InsertPendingParams {
                        source: PendingSource::HumanDislike,
                        turn_id: Some(params.turn_id.clone()),
                        session_id: None,
                        agent_id: None,
                        input,
                        response,
                        risk_signals: vec![],
                    })
                    .await
                {
                    Ok(true) => {
                        debug!("Queued human:dislike pending badcase for turn {}", params.turn_id)
                    }
                    Ok(false) => debug!(
                        "human:dislike pending badcase already queued for turn {}",
                        params.turn_id
                    ),
                    Err(e) => warn!(
                        "Failed to queue human:dislike pending badcase for turn {}: {}",
                        params.turn_id, e
                    ),
                }
            }
        }
    }

    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "status": "recorded",
            "turn_id": params.turn_id,
            "vote": vote.as_str(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::state_tests::make_test_state_with_store;
    use crate::gateway::GatewayConfig;

    fn make_req(id: &str, params: serde_json::Value) -> WsRequest {
        WsRequest {
            frame_type: "req".to_string(),
            id: id.to_string(),
            method: "feedback.vote".to_string(),
            params: Some(params),
        }
    }

    #[tokio::test]
    async fn up_vote_records_feedback() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let res = handle_feedback_vote(
            &make_req(
                "r1",
                serde_json::json!({ "turn_id": "t1", "vote": "up", "comment": "great" }),
            ),
            &state,
        )
        .await;
        assert!(res.ok, "unexpected error: {:?}", res.error);
        assert_eq!(res.payload.as_ref().unwrap()["vote"], "up");

        let vote = state
            .infra
            .feedback_store
            .as_ref()
            .unwrap()
            .get_vote("t1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vote.vote, FeedbackVoteKind::Up);
        assert_eq!(vote.comment.as_deref(), Some("great"));
    }

    #[tokio::test]
    async fn invalid_vote_rejected() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let res = handle_feedback_vote(
            &make_req("r1", serde_json::json!({ "turn_id": "t1", "vote": "maybe" })),
            &state,
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn empty_turn_id_rejected() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let res = handle_feedback_vote(
            &make_req("r1", serde_json::json!({ "turn_id": "  ", "vote": "up" })),
            &state,
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.error.as_ref().unwrap().code, "INVALID_REQUEST");
    }

    #[tokio::test]
    async fn down_vote_seeds_pending_badcase() {
        let state = Arc::new(make_test_state_with_store(GatewayConfig::default()).await);
        let res = handle_feedback_vote(
            &make_req(
                "r1",
                serde_json::json!({
                    "turn_id": "t2",
                    "vote": "down",
                    "input": "user text",
                    "response": "assistant reply",
                }),
            ),
            &state,
        )
        .await;
        assert!(res.ok, "unexpected error: {:?}", res.error);

        let pending = state
            .infra
            .pending_badcase_store
            .as_ref()
            .unwrap()
            .list_pending(crate::eval::PendingStatus::Pending, 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source, PendingSource::HumanDislike);
        assert_eq!(pending[0].input, "user text");
        assert_eq!(pending[0].response, "assistant reply");

        // A second identical down vote dedups.
        let res2 = handle_feedback_vote(
            &make_req(
                "r2",
                serde_json::json!({
                    "turn_id": "t2",
                    "vote": "down",
                    "input": "user text",
                    "response": "assistant reply",
                }),
            ),
            &state,
        )
        .await;
        assert!(res2.ok);
        let pending2 = state
            .infra
            .pending_badcase_store
            .as_ref()
            .unwrap()
            .list_pending(crate::eval::PendingStatus::Pending, 10)
            .await
            .unwrap();
        assert_eq!(pending2.len(), 1, "down vote should dedup on input||response");
    }

    #[tokio::test]
    async fn vote_without_store_errors() {
        // make_test_state leaves stores as None.
        let state =
            Arc::new(crate::gateway::state_tests::make_test_state(GatewayConfig::default()).await);
        let res = handle_feedback_vote(
            &make_req("r1", serde_json::json!({ "turn_id": "t1", "vote": "up" })),
            &state,
        )
        .await;
        assert!(!res.ok);
        assert_eq!(res.error.as_ref().unwrap().code, "FEEDBACK_UNAVAILABLE");
    }
}
