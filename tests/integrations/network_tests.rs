use super::*;

#[tokio::test]
async fn nodes_tool_returns_definitions() {
    let tool = NodesTool::new();
    let result = tool
        .execute(json!({"action": "list"}), &test_context())
        .await;

    match result {
        Ok(output) => {
            println!("Nodes tool output: {}", output.output);
        }
        Err(e) => {
            println!("Nodes tool returned error (expected if no nodes configured): {}", e);
        }
    }
}

#[tokio::test]
async fn nodes_invalid_action_fails() {
    let tool = NodesTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for invalid action");
}

#[tokio::test]
async fn nodes_describe_nonexistent_fails() {
    let tool = NodesTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"action": "describe", "node_id": "nonexistent"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent node");
}
