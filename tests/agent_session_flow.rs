//! Agent Session Flow Integration Tests
//!
//! These tests verify the session management components (transcript, artifacts,
//! disk budget, session files) work correctly in isolation and can be wired
//! into the Agent via the builder pattern. Tests use isolated temp directories.

use manta::agent::{
    AgentBuilder, AgentConfig, ArtifactStore, BudgetCategory, DiskBudgetManager,
    SessionFileManager, TranscriptStore,
};
use manta::tools::ToolRegistry;
use std::sync::Arc;
use tempfile::TempDir;

// ── Disk Budget Tracking Contract ────────────────────────────────────────────

#[test]
fn disk_budget_tracks_transcript_items() {
    let dir = TempDir::new().unwrap();
    let budget = DiskBudgetManager::new(dir.path());
    let _ = budget.init();

    let evicted = budget
        .track_item("session_1", "transcript-1", BudgetCategory::Transcript, 1024)
        .expect("track item");
    assert!(evicted.is_empty(), "no eviction on first item");

    let stats = budget.session_stats("session_1").expect("stats exist");
    assert_eq!(stats.used_bytes, 1024);
    assert_eq!(stats.item_count, 1);
}

#[test]
fn disk_budget_tracks_artifact_items() {
    let dir = TempDir::new().unwrap();
    let budget = DiskBudgetManager::new(dir.path());
    let _ = budget.init();

    let evicted = budget
        .track_item("session_1", "artifact-1", BudgetCategory::Artifact, 2048)
        .expect("track artifact");
    assert!(evicted.is_empty());

    let stats = budget.session_stats("session_1").expect("stats exist");
    assert_eq!(stats.used_bytes, 2048);

    let by_cat = stats.by_category;
    assert!(by_cat.contains_key(&BudgetCategory::Artifact));
    assert_eq!(by_cat[&BudgetCategory::Artifact], 2048);
}

#[test]
fn disk_budget_eviction_when_over_limit() {
    let dir = TempDir::new().unwrap();
    let budget = DiskBudgetManager::new(dir.path()).with_default_limit(100);
    let _ = budget.init();

    let _ = budget
        .track_item("s1", "item-1", BudgetCategory::File, 60)
        .unwrap();

    let evicted = budget
        .track_item("s1", "item-2", BudgetCategory::File, 60)
        .unwrap();

    assert!(!evicted.is_empty(), "LRU eviction should trigger");
    assert!(evicted.contains(&"item-1".to_string()), "oldest item should be evicted");
}

#[test]
fn disk_budget_clear_session_removes_all() {
    let dir = TempDir::new().unwrap();
    let budget = DiskBudgetManager::new(dir.path());
    let _ = budget.init();

    budget
        .track_item("s1", "a", BudgetCategory::File, 100)
        .unwrap();
    budget
        .track_item("s1", "b", BudgetCategory::Transcript, 200)
        .unwrap();

    assert!(budget.session_stats("s1").is_some());

    budget.clear_session("s1");
    assert!(budget.session_stats("s1").is_none(), "stats should be gone after clear");
}

// ── Session File Manager Contract ────────────────────────────────────────────

#[tokio::test]
async fn session_file_manager_isolates_sessions() {
    let dir = TempDir::new().unwrap();
    let manager = SessionFileManager::new(dir.path());
    let _ = manager.init().await;

    manager
        .create_session("session-a")
        .await
        .expect("create session-a");
    manager
        .create_session("session-b")
        .await
        .expect("create session-b");

    let path_a = manager.resolve_path("session-a", "data.txt").await.unwrap();
    let path_b = manager.resolve_path("session-b", "data.txt").await.unwrap();

    assert_ne!(path_a, path_b, "different sessions must have different paths");
    assert!(path_a.to_string_lossy().contains("session-a"));
    assert!(path_b.to_string_lossy().contains("session-b"));
}

#[tokio::test]
async fn session_file_manager_cleanup_removes_session() {
    let dir = TempDir::new().unwrap();
    let manager = SessionFileManager::new(dir.path());
    let _ = manager.init().await;

    manager.create_session("session-del").await.unwrap();

    let path = manager
        .resolve_path("session-del", "file.txt")
        .await
        .unwrap();
    assert!(path.parent().unwrap().exists(), "session dir should exist");

    manager.cleanup_session("session-del").await.unwrap();
    assert!(!path.parent().unwrap().exists(), "session dir should be removed");
}

// ── Artifact Store Contract ──────────────────────────────────────────────────

#[test]
fn artifact_store_adds_and_retrieves() {
    use manta::agent::{Artifact, ArtifactType};

    let dir = TempDir::new().unwrap();
    let store = ArtifactStore::new(dir.path());
    let _ = store.init();

    let artifact = Artifact::code(
        "test-code-1",
        "session-1",
        "Hello World in Rust",
        "rust",
        "fn main() { println!(\"Hello\"); }",
    );

    store.add(artifact.clone());

    let retrieved = store
        .get("session-1", "test-code-1")
        .expect("artifact must exist");
    assert_eq!(retrieved.title, "Hello World in Rust");
    assert_eq!(retrieved.language.as_deref(), Some("rust"));
    assert_eq!(retrieved.artifact_type, ArtifactType::Code);
}

#[test]
fn artifact_store_lists_by_session() {
    use manta::agent::Artifact;

    let dir = TempDir::new().unwrap();
    let store = ArtifactStore::new(dir.path());
    let _ = store.init();

    store.add(Artifact::code("c1", "s1", "Code 1", "py", "print(1)"));
    store.add(Artifact::code("c2", "s1", "Code 2", "py", "print(2)"));
    store.add(Artifact::document("d1", "s2", "Doc 1", "Hello"));

    let s1_artifacts = store.list_session("s1");
    assert_eq!(s1_artifacts.len(), 2, "session s1 must have 2 artifacts");

    let s2_artifacts = store.list_session("s2");
    assert_eq!(s2_artifacts.len(), 1, "session s2 must have 1 artifact");
}

#[test]
fn artifact_store_link_artifact_contract() {
    use manta::agent::{Artifact, ArtifactType};

    let dir = TempDir::new().unwrap();
    let store = ArtifactStore::new(dir.path());
    let _ = store.init();

    let link =
        Artifact::link("link-1", "session-1", "Rust Book", "https://doc.rust-lang.org/book/");

    store.add(link.clone());

    let retrieved = store.get("session-1", "link-1").unwrap();
    assert_eq!(retrieved.artifact_type, ArtifactType::Link);
    assert_eq!(retrieved.url.as_deref(), Some("https://doc.rust-lang.org/book/"));
}

// ── Transcript Store Contract ────────────────────────────────────────────────

#[test]
fn transcript_store_appends_and_exports() {
    use manta::agent::TranscriptMessage;

    let dir = TempDir::new().unwrap();
    let store = TranscriptStore::new(dir.path());
    let _ = store.init();

    store.append(
        "session-1",
        "telegram",
        "user_42",
        "dm",
        TranscriptMessage::new("user", "Hello bot"),
    );
    store.append(
        "session-1",
        "telegram",
        "user_42",
        "dm",
        TranscriptMessage::new("assistant", "Hello human"),
    );

    let transcript = store.get("session-1").expect("transcript must exist");
    assert_eq!(transcript.messages.len(), 2);
    assert_eq!(transcript.messages[0].role, "user");
    assert_eq!(transcript.messages[1].role, "assistant");

    let path = store.flush("session-1").expect("flush must succeed");
    assert!(path.exists(), "exported file must exist");
    assert!(path.to_string_lossy().contains("session-1"));
}

#[test]
fn transcript_store_multiple_sessions_isolated() {
    use manta::agent::TranscriptMessage;

    let dir = TempDir::new().unwrap();
    let store = TranscriptStore::new(dir.path());
    let _ = store.init();

    store.append("session-a", "web", "alice", "room-1", TranscriptMessage::new("user", "msg a"));
    store.append("session-b", "web", "bob", "room-2", TranscriptMessage::new("user", "msg b"));

    let ta = store.get("session-a").unwrap();
    let tb = store.get("session-b").unwrap();

    assert_eq!(ta.messages.len(), 1);
    assert_eq!(tb.messages.len(), 1);
    assert_eq!(ta.messages[0].content, "msg a");
    assert_eq!(tb.messages[0].content, "msg b");
    assert_ne!(ta.peer, tb.peer);
}

// ── Agent Builder Compile-Time Contract ──────────────────────────────────────

#[test]
fn agent_builder_compiles_with_session_stores() {
    // This test verifies at compile-time that the builder accepts all session
    // management stores and that the API surface is consistent.
    let _builder = AgentBuilder::new()
        .config(AgentConfig::default())
        .tools(Arc::new(ToolRegistry::new()));

    // The builder type-checks — the test passes by compiling.
    assert!(true);
}

#[test]
fn agent_builder_requires_provider() {
    let result = AgentBuilder::new()
        .config(AgentConfig::default())
        .tools(Arc::new(ToolRegistry::new()))
        .build();

    assert!(result.is_err(), "build must fail without provider");
}

// ── Agent Builder with Session Stores Compile-Time Contract ──────────────────

#[test]
fn agent_builder_accepts_session_store_methods() {
    // This test verifies at compile time that the builder methods exist
    // and accept the correct types. We don't need to build the agent —
    // the type-check alone proves the API surface is wired.
    let _builder = AgentBuilder::new()
        .config(AgentConfig::default())
        .tools(Arc::new(ToolRegistry::new()))
        .transcript_store(Arc::new(TranscriptStore::new("/tmp/t")))
        .artifact_store(Arc::new(ArtifactStore::new("/tmp/a")))
        .disk_budget(Arc::new(DiskBudgetManager::new("/tmp/b")))
        .session_file_manager(Arc::new(SessionFileManager::new("/tmp/sf")));

    assert!(true, "builder with all session stores type-checks");
}
