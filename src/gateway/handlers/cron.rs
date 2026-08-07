use std::sync::Arc;

use ::cron::Schedule;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};

use crate::gateway::GatewayState;
use crate::gateway::*;

// ── Cron job management
// ───────────────────────────────────────────────────────

#[allow(dead_code)]
/// `GET /api/v1/cron` — list all scheduled jobs.
pub async fn list_cron_jobs_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(scheduler) => {
            let jobs = scheduler.lock().await.list_jobs().await;
            Json(serde_json::json!({ "jobs": jobs, "count": jobs.len() })).into_response()
        }
        None => Json(serde_json::json!({ "jobs": [], "count": 0 })).into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron` — create a new cron job.
pub async fn add_cron_job_handler(
    State(state): State<Arc<GatewayState>>,
    Json(req): Json<AddCronJobRequest>,
) -> impl IntoResponse {
    use std::str::FromStr;

    use crate::cron::cron::{CronJob, ExecutionTarget, Schedule as CronSchedule};

    let schedule = match cron::Schedule::from_str(&req.schedule) {
        Ok(_) => CronSchedule::Cron {
            expression: req.schedule.clone(),
            timezone: None,
            stagger_ms: None,
        },
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid cron expression: {}", e) })),
            )
                .into_response();
        }
    };

    let job_id = uuid::Uuid::new_v4().to_string();
    let job = CronJob::new(
        job_id.clone(),
        req.name.clone(),
        schedule,
        ExecutionTarget::shell(req.command),
    );

    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(scheduler) => match scheduler.lock().await.add_job(job).await {
            Ok(()) => Json(serde_json::json!({
                "success": true,
                "id": job_id,
                "name": req.name,
            }))
            .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to add job: {}", e) })),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `DELETE /api/v1/cron/:id` — remove a cron job.
pub async fn remove_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(scheduler) => match scheduler.lock().await.remove_job(&id).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id })).into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron/:id/enable` — enable a cron job.
pub async fn enable_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(scheduler) => match scheduler.lock().await.set_job_enabled(&id, true).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "enabled": true }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron/:id/disable` — disable a cron job.
pub async fn disable_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(scheduler) => match scheduler.lock().await.set_job_enabled(&id, false).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "enabled": false }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `POST /api/v1/cron/:id/run` — trigger a cron job immediately.
pub async fn trigger_cron_job_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(scheduler) => match scheduler.lock().await.trigger_job(&id).await {
            Ok(()) => Json(serde_json::json!({ "success": true, "id": id, "triggered": true }))
                .into_response(),
            Err(e) => {
                (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": format!("{}", e) })))
                    .into_response()
            }
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[allow(dead_code)]
/// `GET /api/v1/cron/:id/logs` — return job state / last-run info.
pub async fn cron_job_logs_handler(
    Path(id): Path<String>,
    State(state): State<Arc<GatewayState>>,
) -> impl IntoResponse {
    match state.scheduler.cron_scheduler.read().await.clone() {
        Some(scheduler) => match scheduler.lock().await.get_job(&id).await {
            Some(job) => Json(serde_json::json!({
                "id": job.id,
                "name": job.name,
                "enabled": job.enabled,
                "run_count": job.state.run_count,
                "last_run_at": job.state.last_run_at,
                "next_run_at": job.state.next_run_at,
                "last_error": job.state.last_error,
                "consecutive_errors": job.state.consecutive_errors,
            }))
            .into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("Job '{}' not found", id) })),
            )
                .into_response(),
        },
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Cron scheduler not running" })),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cron::cron::CronScheduler;
    use crate::gateway::state_tests::make_test_state;
    use crate::gateway::GatewayConfig;

    async fn body_json(resp: axum::response::Response) -> (StatusCode, serde_json::Value) {
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn add_req(name: &str, schedule: &str) -> Json<AddCronJobRequest> {
        Json(AddCronJobRequest {
            name: name.into(),
            schedule: schedule.into(),
            command: "echo hi".into(),
        })
    }

    /// make_test_state leaves `cron_scheduler == None`: every mutating handler
    /// must 503 and list must report zero jobs.
    #[tokio::test]
    async fn all_handlers_without_scheduler() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (status, body) = body_json(
            list_cron_jobs_handler(State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["count"].as_u64(), Some(0));

        let (status, _) = body_json(
            add_cron_job_handler(State(state.clone()), add_req("j", "* * * * * *"))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "add should 503");

        let (status, _) = body_json(
            remove_cron_job_handler(Path("j".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "remove should 503");

        let (status, _) = body_json(
            enable_cron_job_handler(Path("j".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "enable should 503");

        let (status, _) = body_json(
            disable_cron_job_handler(Path("j".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "disable should 503");

        let (status, _) = body_json(
            trigger_cron_job_handler(Path("j".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "trigger should 503");

        let (status, _) = body_json(
            cron_job_logs_handler(Path("j".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "logs should 503");
    }

    /// Inject a scheduler (channel left open but not started) so the
    /// command-send paths succeed.
    #[tokio::test]
    async fn add_job_valid_and_invalid_schedule() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (sched, rx) = CronScheduler::new();
        *state.scheduler.cron_scheduler.write().await =
            Some(Arc::new(tokio::sync::Mutex::new(sched)));
        let _keep_rx_alive = rx;

        let (status, body) = body_json(
            add_cron_job_handler(State(state.clone()), add_req("nightly", "0 0 * * * *"))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"].as_bool(), Some(true));
        assert!(body["id"].as_str().is_some());

        let (status, _) = body_json(
            add_cron_job_handler(State(state), add_req("bad", "not-a-cron"))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn remove_enable_disable_trigger_with_scheduler() {
        let state = Arc::new(make_test_state(GatewayConfig::default()).await);
        let (sched, rx) = CronScheduler::new();
        *state.scheduler.cron_scheduler.write().await =
            Some(Arc::new(tokio::sync::Mutex::new(sched)));
        let _keep_rx_alive = rx;

        let (status, body) = body_json(
            remove_cron_job_handler(Path("ghost".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap_or_default());

        let (status, _) = body_json(
            enable_cron_job_handler(Path("ghost".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "enable should succeed on send");

        let (status, _) = body_json(
            disable_cron_job_handler(Path("ghost".into()), State(state.clone()))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "disable should succeed on send");

        let (status, _) = body_json(
            trigger_cron_job_handler(Path("ghost".into()), State(state))
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "trigger should succeed on send");
    }
}
