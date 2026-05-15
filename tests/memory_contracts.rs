//! Memory Storage Contract Tests
//!
//! These tests verify that Memory, ChatMessage, MemoryId, and DatabaseStore
//! maintain stable serialization contracts and that in-memory store operations
//! roundtrip correctly. Tests use isolated temporary directories.

use manta::memory::*;
use serde_json::json;
use std::time::SystemTime;

// ── MemoryId Serialization Contract ──────────────────────────────────────────

#[test]
fn memory_id_serializes_to_string() {
    let id = MemoryId::new("test-123");
    let json = serde_json::to_value(&id).unwrap();
    assert_eq!(json, "test-123");
}

#[test]
fn memory_id_roundtrips_through_json() {
    let original = MemoryId::generate();
    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: MemoryId = serde_json::from_str(&json).unwrap();
    assert_eq!(original.0, roundtripped.0);
}

// ── Memory Serialization Contract ────────────────────────────────────────────

#[test]
fn memory_serializes_to_expected_shape() {
    let mem = Memory::new("user_1", "Rust is fast", "fact").with_importance_score(0.9);

    let json = serde_json::to_value(&mem).unwrap();
    assert!(json.get("id").is_some(), "missing 'id'");
    assert!(json.get("user_id").is_some(), "missing 'user_id'");
    assert!(json.get("content").is_some(), "missing 'content'");
    assert!(json.get("memory_type").is_some(), "missing 'memory_type'");
    assert!(json.get("created_at").is_some(), "missing 'created_at'");
    assert!(json.get("importance_score").is_some(), "missing 'importance_score'");
    assert!(json.get("source").is_some(), "missing 'source'");

    assert_eq!(json["user_id"], "user_1");
    assert_eq!(json["content"], "Rust is fast");
    assert_eq!(json["memory_type"], "fact");
    let score = json["importance_score"].as_f64().unwrap();
    assert!((score - 0.9).abs() < 0.001, "importance_score should be ~0.9, got {}", score);
    assert_eq!(json["source"], "agent"); // default
}

#[test]
fn memory_roundtrips_through_json() {
    let original = Memory::new("alice", "Prefers dark mode", "preference")
        .with_conversation("conv_42")
        .with_metadata(json!({"theme": "dark"}))
        .with_importance_score(0.75);

    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: Memory = serde_json::from_str(&json).unwrap();

    assert_eq!(original.id.0, roundtripped.id.0);
    assert_eq!(original.user_id, roundtripped.user_id);
    assert_eq!(original.content, roundtripped.content);
    assert_eq!(original.memory_type, roundtripped.memory_type);
    assert_eq!(original.conversation_id, roundtripped.conversation_id);
    assert_eq!(original.importance_score, roundtripped.importance_score);
    assert_eq!(original.metadata, roundtripped.metadata);
}

#[test]
fn memory_default_importance_score() {
    let mem = Memory::new("u1", "content", "fact");
    assert_eq!(mem.importance_score, 0.5);
}

#[test]
fn memory_default_source() {
    let mem = Memory::new("u1", "content", "fact");
    assert_eq!(mem.source, "agent");
}

// ── ChatMessage Serialization Contract ───────────────────────────────────────

#[test]
fn chat_message_serializes_to_expected_shape() {
    let msg = ChatMessage::new("conv_1", "user_1", "user", "Hello");
    let json = serde_json::to_value(&msg).unwrap();

    assert!(json.get("id").is_some(), "missing 'id'");
    assert!(json.get("conversation_id").is_some(), "missing 'conversation_id'");
    assert!(json.get("user_id").is_some(), "missing 'user_id'");
    assert!(json.get("role").is_some(), "missing 'role'");
    assert!(json.get("content").is_some(), "missing 'content'");
    assert!(json.get("created_at").is_some(), "missing 'created_at'");

    assert_eq!(json["conversation_id"], "conv_1");
    assert_eq!(json["role"], "user");
}

#[test]
fn chat_message_roundtrips_through_json() {
    let original = ChatMessage::new("conv_99", "bob", "assistant", "How can I help?")
        .with_metadata(json!({"tokens": 42}));

    let json = serde_json::to_string(&original).unwrap();
    let roundtripped: ChatMessage = serde_json::from_str(&json).unwrap();

    assert_eq!(original.id, roundtripped.id);
    assert_eq!(original.conversation_id, roundtripped.conversation_id);
    assert_eq!(original.role, roundtripped.role);
    assert_eq!(original.content, roundtripped.content);
    assert_eq!(original.metadata, roundtripped.metadata);
}

#[test]
fn chat_message_ids_are_unique() {
    let msg1 = ChatMessage::new("c1", "u1", "user", "a");
    let msg2 = ChatMessage::new("c1", "u1", "user", "b");
    assert_ne!(msg1.id, msg2.id);
}

// ── DatabaseStore (In-Memory) Contract ───────────────────────────────────────

#[tokio::test]
async fn database_store_memory_lifecycle() {
    let store = DatabaseStore::new_in_memory().await.expect("create in-memory store");

    // Store a memory
    let mem = Memory::new("alice", "Likes coffee", "preference");
    let id = store.store(mem.clone()).await.expect("store memory");

    // Retrieve by ID
    let retrieved = store.get(&id).await.expect("get memory");
    assert!(retrieved.is_some(), "stored memory must be retrievable");
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.content, "Likes coffee");
    assert_eq!(retrieved.user_id, "alice");
}

#[tokio::test]
async fn database_store_memory_search_contract() {
    let store = DatabaseStore::new_in_memory().await.expect("create store");

    let mem1 = Memory::new("alice", "I love Rust programming", "preference");
    let mem2 = Memory::new("alice", "Python is also nice", "preference");

    store.store(mem1).await.unwrap();
    store.store(mem2).await.unwrap();

    // Search for "Rust" using MemoryQuery
    let query = MemoryQuery::new()
        .for_user("alice")
        .with_content("Rust");
    let results = store.search(query).await.expect("search memories");
    assert!(!results.is_empty(), "search must find the memory");
}

#[tokio::test]
async fn database_store_chat_history_contract() {
    let store = DatabaseStore::new_in_memory().await.expect("create store");

    // Store multiple messages
    let messages = vec![
        ChatMessage::new("conv_1", "user_1", "user", "Hello"),
        ChatMessage::new("conv_1", "user_1", "assistant", "Hi there!"),
        ChatMessage::new("conv_1", "user_1", "user", "How are you?"),
    ];

    for msg in &messages {
        store.store_message(msg.clone()).await.expect("store message");
    }

    // Get conversation history
    let history = store
        .get_conversation_history("conv_1", 10)
        .await
        .expect("get history");
    assert_eq!(history.len(), 3, "must retrieve all messages");

    // Limit works
    let limited = store
        .get_conversation_history("conv_1", 2)
        .await
        .expect("get limited history");
    assert_eq!(limited.len(), 2, "limit must be respected");
}

#[tokio::test]
async fn database_store_delete_conversation() {
    let store = DatabaseStore::new_in_memory().await.expect("create store");

    store
        .store_message(ChatMessage::new("conv_del", "u1", "user", "msg1"))
        .await
        .unwrap();
    store
        .store_message(ChatMessage::new("conv_del", "u1", "assistant", "msg2"))
        .await
        .unwrap();

    // Verify exists
    let before = store
        .get_conversation_history("conv_del", 10)
        .await
        .unwrap();
    assert_eq!(before.len(), 2);

    // Delete
    store.delete_conversation("conv_del").await.expect("delete conversation");

    // Verify gone
    let after = store
        .get_conversation_history("conv_del", 10)
        .await
        .unwrap();
    assert!(after.is_empty(), "conversation must be empty after deletion");
}

#[tokio::test]
async fn database_store_user_conversations() {
    let store = DatabaseStore::new_in_memory().await.expect("create store");

    // Messages for different users and conversations
    store
        .store_message(ChatMessage::new("conv_a", "alice", "user", "hi"))
        .await
        .unwrap();
    store
        .store_message(ChatMessage::new("conv_b", "alice", "user", "hello"))
        .await
        .unwrap();
    store
        .store_message(ChatMessage::new("conv_c", "bob", "user", "hey"))
        .await
        .unwrap();

    // Get Alice's conversations
    let alice_convs = store
        .get_user_conversations("alice", 10)
        .await
        .expect("get user conversations");
    assert_eq!(alice_convs.len(), 2, "alice must have 2 conversations");
    assert!(alice_convs.contains(&"conv_a".to_string()));
    assert!(alice_convs.contains(&"conv_b".to_string()));

    // Get Bob's conversations
    let bob_convs = store
        .get_user_conversations("bob", 10)
        .await
        .expect("get bob conversations");
    assert_eq!(bob_convs.len(), 1);
    assert!(bob_convs.contains(&"conv_c".to_string()));
}

#[tokio::test]
async fn database_store_last_conversation() {
    let store = DatabaseStore::new_in_memory().await.expect("create store");

    // No conversations yet
    let none = store.get_last_conversation("alice").await.unwrap();
    assert!(none.is_none());

    // Add a conversation
    store
        .store_message(ChatMessage::new("conv_first", "alice", "user", "hello"))
        .await
        .unwrap();

    let last = store.get_last_conversation("alice").await.unwrap();
    assert_eq!(last, Some("conv_first".to_string()));
}

#[tokio::test]
async fn database_store_stats_contract() {
    let store = DatabaseStore::new_in_memory().await.expect("create store");

    // Empty store stats
    let stats = store.stats().await.expect("get stats");
    assert_eq!(stats.total_count, 0);

    // After adding
    store
        .store(Memory::new("u1", "content", "fact"))
        .await
        .unwrap();
    let stats = store.stats().await.expect("get stats");
    assert_eq!(stats.total_count, 1);
}

// ── Cosine Similarity Contract ───────────────────────────────────────────────

#[test]
fn cosine_similarity_identical_vectors() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![1.0, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - 1.0).abs() < 1e-6, "identical vectors must have similarity 1.0");
}

#[test]
fn cosine_similarity_orthogonal_vectors() {
    let a = vec![1.0, 0.0];
    let b = vec![0.0, 1.0];
    let sim = cosine_similarity(&a, &b);
    assert!(sim.abs() < 1e-6, "orthogonal vectors must have similarity 0.0");
}

#[test]
fn cosine_similarity_opposite_vectors() {
    let a = vec![1.0, 0.0];
    let b = vec![-1.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - (-1.0)).abs() < 1e-6, "opposite vectors must have similarity -1.0");
}

#[test]
fn cosine_similarity_empty_vectors() {
    let a: Vec<f32> = vec![];
    let b: Vec<f32> = vec![];
    let sim = cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0, "empty vectors must return 0.0");
}

#[test]
fn cosine_similarity_mismatched_lengths() {
    let a = vec![1.0, 0.0];
    let b = vec![1.0, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0, "mismatched lengths must return 0.0");
}
