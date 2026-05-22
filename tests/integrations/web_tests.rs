use super::*;

#[tokio::test]
async fn web_fetch_tool_fetches_example_com() {
    let tool = WebFetchTool::new();
    let result = tool
        .execute(json!({"url": "https://example.com"}), &test_context())
        .await;

    match result {
        Ok(output) => {
            if output.success {
                assert!(
                    output.output.contains("Example Domain")
                        || output.output.to_lowercase().contains("example")
                        || output.output.is_empty(),
                    "Expected example.com content, got: {}",
                    output.output
                );
            } else {
                println!("web_fetch returned error: {:?}", output.error);
            }
        }
        Err(e) => {
            println!("web_fetch failed (network may be unavailable): {}", e);
        }
    }
}

#[tokio::test]
async fn web_search_tool_duckduckgo() {
    let tool = WebSearchTool::new();
    let result = tool
        .execute(json!({"query": "Rust programming language", "limit": 3}), &test_context())
        .await;

    match result {
        Ok(output) => {
            assert!(!output.output.is_empty(), "Expected search results, got empty output");
            println!("WebSearch results: {}", output.output);
        }
        Err(e) => {
            println!("WebSearchTool failed (network may be unavailable): {}", e);
        }
    }
}

#[tokio::test]
async fn web_fetch_invalid_url_fails() {
    let tool = WebFetchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({"url": "not-a-url"}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for invalid URL");
}

#[tokio::test]
async fn web_fetch_unsupported_scheme_fails() {
    let tool = WebFetchTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"url": "ftp://example.com"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for unsupported scheme");
    assert!(output.error.as_ref().unwrap().contains("scheme"));
}

#[tokio::test]
async fn web_fetch_missing_url_validation_error() {
    let tool = WebFetchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing url");
}

#[tokio::test]
async fn web_search_missing_query_validation_error() {
    let tool = WebSearchTool::new();
    let ctx = test_context();
    let result = tool.execute(json!({}), &ctx).await;
    assert!(result.is_err(), "Expected validation error for missing query");
}

#[tokio::test]
async fn web_search_query_too_long_fails() {
    let tool = WebSearchTool::new();
    let ctx = test_context();
    let long_query = "a".repeat(501);
    let result = tool.execute(json!({"query": long_query}), &ctx).await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for query too long");
    assert!(output.error.as_ref().unwrap().contains("too long"));
}

#[tokio::test]
async fn web_search_returns_structured_results() {
    let tool = WebSearchTool::new();
    let ctx = test_context();
    let result = tool
        .execute(json!({"query": "Rust programming language", "limit": 3}), &ctx)
        .await;

    match result {
        Ok(output) => {
            if output.success {
                let results = output
                    .data
                    .as_ref()
                    .and_then(|d| d.get("results"))
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                assert!(!results.is_empty(), "Expected structured results array");
            } else {
                println!(
                    "Web search returned error (network may be unavailable): {:?}",
                    output.error
                );
            }
        }
        Err(e) => {
            println!("Web search failed (network may be unavailable): {}", e);
        }
    }
}
