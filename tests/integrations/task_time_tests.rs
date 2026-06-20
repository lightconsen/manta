use super::*;

#[tokio::test]
async fn time_tool_returns_timestamp() {
    let tool = TimeTool::new();
    let result = tool
        .execute(json!({"action": "now"}), &test_context())
        .await
        .expect("time tool should succeed");

    assert!(
        result.output.contains("2025") || result.output.contains("2026"),
        "Expected current year in time output, got: {}",
        result.output
    );
}

#[tokio::test]
async fn todo_tool_adds_and_lists() {
    let tool = TodoTool::new();
    let ctx = test_context();

    let _ = tool
        .execute(
            json!({
                "action": "create",
                "content": "test todo item"
            }),
            &ctx,
        )
        .await
        .expect("todo create should succeed");

    let result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .expect("todo list should succeed");

    assert!(
        result.output.contains("test todo item"),
        "Expected todo item in list, got: {}",
        result.output
    );
}

#[tokio::test]
async fn cron_tool_list_without_scheduler() {
    let tool = CronTool::new();
    let result = tool
        .execute(json!({"action": "list"}), &test_context())
        .await;

    assert!(result.is_ok(), "Expected Ok result when scheduler not set");
    let r = result.unwrap();
    assert!(!r.success, "Expected success=false when scheduler not set");
    let err_msg = r.error.expect("Expected error message");
    assert!(
        err_msg.contains("scheduler")
            || err_msg.contains("not initialized")
            || err_msg.contains("Cron scheduler not available"),
        "Expected scheduler-related error, got: {}",
        err_msg
    );
}

#[tokio::test]
async fn todo_updates_status() {
    let tool = TodoTool::new();
    let ctx = test_context();

    let create_result = tool
        .execute(json!({"action": "create", "content": "status test"}), &ctx)
        .await
        .expect("create failed");
    let task_id = create_result
        .data
        .as_ref()
        .and_then(|d| d.get("task_id"))
        .and_then(|v| v.as_str())
        .expect("Missing task_id")
        .to_string();

    let update_result = tool
        .execute(json!({"action": "update", "task_id": task_id, "status": "completed"}), &ctx)
        .await
        .expect("update failed");
    assert!(update_result.success, "Update should succeed");

    let list_result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .expect("list failed");
    let tasks = list_result
        .data
        .as_ref()
        .and_then(|d| d.get("tasks"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let updated = tasks.iter().any(|t| {
        t.get("id").and_then(|v| v.as_str()) == Some(&task_id)
            && t.get("status").and_then(|v| v.as_str()) == Some("completed")
    });
    assert!(updated, "Expected todo status to be updated to completed");
}

#[tokio::test]
async fn todo_update_nonexistent_fails() {
    let tool = TodoTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({"action": "update", "task_id": "nonexistent-id", "status": "completed"}),
            &ctx,
        )
        .await;
    let is_failed = result.as_ref().map(|o| !o.success).unwrap_or(true);
    assert!(is_failed, "Expected failure for nonexistent task");
}

#[tokio::test]
async fn todo_clears_completed() {
    let tool = TodoTool::new();
    let mut ctx = test_context();
    ctx.identity.conversation_id = format!("todo-clear-{}", std::process::id());

    let r1 = tool
        .execute(json!({"action": "create", "content": "task 1"}), &ctx)
        .await
        .unwrap();
    let id1 = r1
        .data
        .as_ref()
        .unwrap()
        .get("task_id")
        .unwrap()
        .as_str()
        .unwrap();

    let r2 = tool
        .execute(json!({"action": "create", "content": "task 2"}), &ctx)
        .await
        .unwrap();
    let _id2 = r2
        .data
        .as_ref()
        .unwrap()
        .get("task_id")
        .unwrap()
        .as_str()
        .unwrap();

    let _ = tool
        .execute(json!({"action": "update", "task_id": id1, "status": "completed"}), &ctx)
        .await;

    let clear_result = tool
        .execute(json!({"action": "clear_completed"}), &ctx)
        .await
        .unwrap();
    assert!(clear_result.success);

    let list_result = tool.execute(json!({"action": "list"}), &ctx).await.unwrap();
    let tasks = list_result
        .data
        .as_ref()
        .and_then(|d| d.get("tasks"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert_eq!(tasks.len(), 1, "Expected 1 remaining todo after clearing completed");
}

#[tokio::test]
async fn time_invalid_timezone_fails() {
    let tool = TimeTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "now", "timezone": "Mars/Standard"}), &ctx)
        .await;
    let is_failed = result.as_ref().map(|o| !o.success).unwrap_or(true);
    assert!(is_failed, "Expected failure for invalid timezone");
}

#[tokio::test]
async fn time_format_custom_pattern() {
    let tool = TimeTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "now", "format": "%Y-%m-%d"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    let current_year = chrono::Local::now().format("%Y").to_string();
    assert!(
        output.output.contains(&current_year),
        "Expected output to contain current year, got: {}",
        output.output
    );
}

#[tokio::test]
async fn cron_invalid_expression_fails() {
    let tool = CronTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "action": "create",
                "name": "test-invalid",
                "schedule": "not-a-cron",
                "command": "echo test"
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid cron expression");
}

#[tokio::test]
async fn cron_remove_nonexistent_fails() {
    let tool = CronTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "remove", "name": "nonexistent-job"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent job");
}
