use super::*;

#[tokio::test]
async fn shell_tool_executes_echo() {
    let tool = ShellTool::new();
    let result = tool
        .execute(json!({"command": "echo test-output"}), &test_context())
        .await
        .expect("shell tool should succeed");

    let output = result.output.to_lowercase();
    assert!(
        output.contains("test-output"),
        "Expected 'test-output' in shell output, got: {}",
        output
    );
}

#[tokio::test]
async fn code_exec_tool_runs_python() {
    let tool = CodeExecutionTool::default();
    let result = tool
        .execute(
            json!({
                "language": "python",
                "code": "print('hello-from-python')"
            }),
            &test_context(),
        )
        .await;

    if let Ok(output) = result {
        assert!(
            output.output.contains("hello-from-python"),
            "Expected python output, got: {}",
            output.output
        );
    }
}

#[tokio::test]
async fn process_tool_lists_processes() {
    let tool = ProcessTool::new();
    let result = tool
        .execute(json!({"action": "list"}), &test_context())
        .await
        .expect("process tool should succeed");

    assert!(!result.output.is_empty(), "process tool returned empty content");
}

#[tokio::test]
async fn shell_missing_command_validation_error() {
    let tool = ShellTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing command");
}

#[tokio::test]
async fn shell_nonzero_exit_fails() {
    let tool = ShellTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"command": "exit 42"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonzero exit code");
    assert!(
        output.error.as_ref().unwrap().contains("42")
            || output.error.as_ref().unwrap().contains("Exit code"),
        "Expected exit code in error, got: {:?}",
        output.error
    );
}

#[tokio::test]
async fn shell_pipeline_works() {
    let tool = ShellTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"command": "echo 'hello pipe' | grep pipe"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert!(output.output.contains("hello pipe"));
}

#[tokio::test]
async fn code_exec_forbidden_import_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"code": "import subprocess\nprint('ok')", "language": "python"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for forbidden import");
    assert!(
        output.error.as_ref().unwrap().contains("validation failed")
            || output.error.as_ref().unwrap().contains("forbidden"),
        "Expected validation error, got: {:?}",
        output.error
    );
}

#[tokio::test]
async fn code_exec_dangerous_pattern_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"code": "eval('1+1')", "language": "python"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for dangerous pattern");
}

#[tokio::test]
async fn code_exec_unsupported_language_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"code": "puts 'hello'", "language": "ruby"}), &ctx)
        .await;
    assert!(result.is_err(), "Expected validation error for unsupported language");
}

#[tokio::test]
async fn code_exec_timeout_fails() {
    let tool = CodeExecutionTool::new();
    let ctx = test_context();
    let result = tool
        .execute(
            json!({
                "code": "import time\ntime.sleep(300)",
                "language": "python",
                "timeout": 2
            }),
            &ctx,
        )
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected timeout failure");
    assert!(
        output
            .error
            .as_ref()
            .unwrap()
            .to_lowercase()
            .contains("timed out"),
        "Expected timeout error, got: {:?}",
        output.error
    );
}

#[tokio::test]
async fn process_invalid_action_fails() {
    let tool = ProcessTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "invalid_action"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid action");
}

#[tokio::test]
async fn process_stop_nonexistent_fails() {
    let tool = ProcessTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "stop", "process_id": "nonexistent-id"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent process");
}
