//! Cron + task-scheduler handlers.

use super::*;
pub(super) async fn handle_cron_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let jobs = match state.scheduler.cron_scheduler.read().await.clone() {
        Some(s) => s.lock().await.list_jobs().await,
        None => Vec::new(),
    };
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "jobs": jobs,
            "count": jobs.len(),
        }),
    )
}

pub(super) async fn handle_tasks_schedule(
    req: &WsRequest,
    state: &Arc<GatewayState>,
) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskSchedulePayload {
        id: String,
        name: String,
        #[serde(default)]
        description: String,
        schedule: ScheduleInput,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", content = "value")]
    enum ScheduleInput {
        #[serde(rename = "once")]
        Once(String),
        #[serde(rename = "interval")]
        Interval(u64),
        #[serde(rename = "cron")]
        Cron(String),
    }

    let payload: TaskSchedulePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let schedule = match payload.schedule {
        ScheduleInput::Once(s) => crate::planner::Schedule::once(s),
        ScheduleInput::Interval(seconds) => crate::planner::Schedule::interval(seconds),
        ScheduleInput::Cron(expr) => crate::planner::Schedule::cron(expr),
    };

    let task =
        crate::planner::ScheduledTask::new(payload.id.clone(), payload.name, schedule, vec![])
            .with_description(payload.description);

    let scheduler = scheduler.lock().await;
    match scheduler.add(task).await {
        Ok(()) => WsResponse::ok(
            &req.id,
            serde_json::json!({
                "status": "scheduled",
                "id": payload.id,
            }),
        ),
        Err(e) => WsResponse::err(&req.id, "SCHEDULE_FAILED", format!("{}", e)),
    }
}

pub(super) async fn handle_tasks_list(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    let tasks = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s.lock().await.list().await,
        None => Vec::new(),
    };
    WsResponse::ok(
        &req.id,
        serde_json::json!({
            "tasks": tasks,
            "count": tasks.len(),
        }),
    )
}

pub(super) async fn handle_tasks_delete(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskDeletePayload {
        id: String,
    }
    let payload: TaskDeletePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let scheduler = scheduler.lock().await;
    match scheduler.remove(&payload.id).await {
        Ok(true) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "deleted", "id": payload.id }))
        }
        Ok(false) => {
            WsResponse::err(&req.id, "NOT_FOUND", format!("Task '{}' not found", payload.id))
        }
        Err(e) => WsResponse::err(&req.id, "DELETE_FAILED", format!("{}", e)),
    }
}

pub(super) async fn handle_tasks_enable(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskEnablePayload {
        id: String,
    }
    let payload: TaskEnablePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let scheduler = scheduler.lock().await;
    match scheduler.enable(&payload.id).await {
        Ok(true) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "enabled", "id": payload.id }))
        }
        Ok(false) => {
            WsResponse::err(&req.id, "NOT_FOUND", format!("Task '{}' not found", payload.id))
        }
        Err(e) => WsResponse::err(&req.id, "ENABLE_FAILED", format!("{}", e)),
    }
}

pub(super) async fn handle_tasks_disable(req: &WsRequest, state: &Arc<GatewayState>) -> WsResponse {
    #[derive(Debug, Deserialize)]
    struct TaskDisablePayload {
        id: String,
    }
    let payload: TaskDisablePayload = match parse_params(req) {
        Ok(p) => p,
        Err(res) => return res,
    };

    let scheduler = match state.scheduler.task_scheduler.read().await.clone() {
        Some(s) => s,
        None => {
            return WsResponse::err(
                &req.id,
                "SCHEDULER_UNAVAILABLE",
                "Task scheduler is not running",
            )
        }
    };

    let scheduler = scheduler.lock().await;
    match scheduler.disable(&payload.id).await {
        Ok(true) => {
            WsResponse::ok(&req.id, serde_json::json!({ "status": "disabled", "id": payload.id }))
        }
        Ok(false) => {
            WsResponse::err(&req.id, "NOT_FOUND", format!("Task '{}' not found", payload.id))
        }
        Err(e) => WsResponse::err(&req.id, "DISABLE_FAILED", format!("{}", e)),
    }
}
