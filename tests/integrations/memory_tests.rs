use super::*;

#[tokio::test]
async fn memory_tool_creates_and_reads() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memory_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url)
        .await
        .expect("Failed to create MemoryTool");
    let ctx = test_context();

    let store_result = tool
        .execute(
            json!({
                "action": "store",
                "content": "e2e-test-memory-value",
                "category": "fact"
            }),
            &ctx,
        )
        .await
        .expect("store failed");

    let stored_id = store_result
        .data
        .as_ref()
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .expect("Missing memory id")
        .to_string();

    let retrieve_result = tool
        .execute(json!({"action": "retrieve", "id": stored_id}), &ctx)
        .await
        .expect("retrieve failed");

    assert!(
        retrieve_result.output.contains("e2e-test-memory-value"),
        "Expected stored memory content, got: {}",
        retrieve_result.output
    );

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_search_tool_searches() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_search_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url)
            .await
            .expect("Failed to create store"),
    );
    let tool = MemorySearchTool::with_store(store.clone());
    let ctx = test_context();

    let memory_tool = MemoryTool::with_store(store.clone())
        .await
        .expect("Failed to create MemoryTool");
    let _ = memory_tool
        .execute(
            json!({
                "action": "store",
                "content": "Rust is a systems programming language",
                "category": "fact"
            }),
            &ctx,
        )
        .await
        .expect("store failed");

    let result = tool
        .execute(json!({"action": "search", "query": "Rust programming"}), &ctx)
        .await
        .expect("search should succeed");

    assert!(
        result.output.contains("Rust") || result.output.contains("programming"),
        "Expected search results, got: {}",
        result.output
    );

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_tool_crud() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_get_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url)
            .await
            .expect("Failed to create store"),
    );
    let tool = MemoryGetTool::with_store(store.clone());
    let ctx = test_context();

    let memory_tool = MemoryTool::with_store(store.clone())
        .await
        .expect("Failed to create MemoryTool");
    let store_result = memory_tool
        .execute(
            json!({
                "action": "store",
                "content": "memory-get-test-content",
                "category": "fact"
            }),
            &ctx,
        )
        .await
        .expect("store failed");

    let memory_id = store_result
        .data
        .as_ref()
        .and_then(|d| d.get("id"))
        .and_then(|v| v.as_str())
        .expect("Missing memory id")
        .to_string();

    let result = tool
        .execute(json!({"action": "retrieve", "id": memory_id}), &ctx)
        .await
        .expect("retrieve should succeed");

    assert!(
        result.output.contains("memory-get-test-content"),
        "Expected memory content, got: {}",
        result.output
    );

    let list_result = tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .expect("list should succeed");
    assert!(
        list_result.output.contains("memory-get-test-content"),
        "Expected memory in list, got: {}",
        list_result.output
    );

    let _ = tool
        .execute(json!({"action": "delete", "id": memory_id}), &ctx)
        .await
        .expect("delete should succeed");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_retrieve_nonexistent_fails() {
    let db_path =
        std::env::temp_dir().join(format!("manta_e2e_memory_neg_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url)
        .await
        .expect("Failed to create MemoryTool");
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "retrieve", "id": "nonexistent-id"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent memory");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_delete_removes_entry() {
    let db_path =
        std::env::temp_dir().join(format!("manta_e2e_memory_del_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url)
        .await
        .expect("Failed to create MemoryTool");
    let ctx = test_context();

    let store_result = tool
        .execute(json!({"action": "store", "content": "to-delete", "category": "test"}), &ctx)
        .await
        .expect("store failed");
    let id = store_result
        .data
        .as_ref()
        .unwrap()
        .get("id")
        .unwrap()
        .as_str()
        .unwrap();

    let del_result = tool
        .execute(json!({"action": "delete", "id": id}), &ctx)
        .await
        .unwrap();
    assert!(del_result.success);

    let retrieve_result = tool
        .execute(json!({"action": "retrieve", "id": id}), &ctx)
        .await
        .unwrap();
    assert!(!retrieve_result.success, "Expected retrieve to fail after delete");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_update_modifies_content() {
    let db_path =
        std::env::temp_dir().join(format!("manta_e2e_memory_upd_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url)
        .await
        .expect("Failed to create MemoryTool");
    let ctx = test_context();

    let store_result = tool
        .execute(json!({"action": "store", "content": "original", "category": "test"}), &ctx)
        .await
        .expect("store failed");
    let id = store_result
        .data
        .as_ref()
        .unwrap()
        .get("id")
        .unwrap()
        .as_str()
        .unwrap();

    let update_result = tool
        .execute(json!({"action": "update", "id": id, "content": "updated"}), &ctx)
        .await
        .unwrap();
    assert!(update_result.success);

    let retrieve_result = tool
        .execute(json!({"action": "retrieve", "id": id}), &ctx)
        .await
        .unwrap();
    assert!(retrieve_result.output.contains("updated"), "Expected updated content");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_invalid_action_fails() {
    let db_path =
        std::env::temp_dir().join(format!("manta_e2e_memory_inv_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let tool = MemoryTool::with_database_url(&db_url)
        .await
        .expect("Failed to create MemoryTool");
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "invalid_action"}), &ctx)
        .await;
    assert!(
        result.is_err() || !result.unwrap().success,
        "Expected failure for invalid action"
    );

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_search_no_results_returns_empty() {
    let db_path =
        std::env::temp_dir().join(format!("manta_e2e_memsearch_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url)
            .await
            .expect("Failed to create store"),
    );
    let tool = MemorySearchTool::with_store(store);
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "search", "query": "xyz_nonexistent_query_12345"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(output.success);
    assert!(output.output.contains("No memories found") || output.output.contains("0"));

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_search_store_then_search() {
    let db_path =
        std::env::temp_dir().join(format!("manta_e2e_memsearch2_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url)
            .await
            .expect("Failed to create store"),
    );
    let tool = MemorySearchTool::with_store(store);
    let ctx = test_context();

    let _ = tool
        .execute(
            json!({"action": "store", "content": "Manta is a great project", "category": "test"}),
            &ctx,
        )
        .await;

    let result = tool
        .execute(json!({"action": "search", "query": "great"}), &ctx)
        .await
        .unwrap();
    assert!(result.success);
    let count = result
        .data
        .as_ref()
        .and_then(|d| d.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert!(count >= 1, "Expected at least 1 search result");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_delete_nonexistent_fails() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memget_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url)
            .await
            .expect("Failed to create store"),
    );
    let tool = MemoryGetTool::with_store(store);
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "delete", "id": "nonexistent-id"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent memory");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_list_returns_all() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memget2_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url)
            .await
            .expect("Failed to create store"),
    );
    let memory_tool = MemoryTool::with_store(store.clone())
        .await
        .expect("Failed to create MemoryTool");
    let get_tool = MemoryGetTool::with_store(store);
    let ctx = test_context();

    for i in 0..3 {
        let _ = memory_tool
            .execute(
                json!({"action": "store", "content": format!("entry {}", i), "category": "test"}),
                &ctx,
            )
            .await;
    }

    let result = get_tool
        .execute(json!({"action": "list"}), &ctx)
        .await
        .unwrap();
    assert!(result.success);
    let count = result
        .data
        .as_ref()
        .and_then(|d| d.get("count"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert_eq!(count, 3, "Expected 3 memories in list");

    let _ = std::fs::remove_file(&db_path);
}

#[tokio::test]
async fn memory_get_update_nonexistent_fails() {
    let db_path = std::env::temp_dir().join(format!("manta_e2e_memget3_{}.db", std::process::id()));
    let db_url = format!("sqlite:/// {}", db_path.display()).replace("/ /", "/");
    let store = Arc::new(
        manta::memory::SqliteMemoryStore::new(&db_url)
            .await
            .expect("Failed to create store"),
    );
    let tool = MemoryGetTool::with_store(store);
    let ctx = test_context();

    let result = tool
        .execute(json!({"action": "update", "id": "nonexistent-id", "content": "new"}), &ctx)
        .await;
    assert!(result.is_ok());
    let output = result.unwrap();
    assert!(!output.success, "Expected failure for nonexistent memory");

    let _ = std::fs::remove_file(&db_path);
}
