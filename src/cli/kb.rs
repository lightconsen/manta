//! Knowledge Base management commands for Syscity CLI.
//!
//! Provides CLI access to KB ingestion: ingest agent documents, list indexed
//! documents, and delete collections.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Subcommand;

use crate::error::Result;
use crate::rag::ingestion::{
    KnowledgeBaseManager, KnowledgeSource, SourceType,
};
use crate::rag::embedding::EmbeddingProvider;
use crate::rag::VectorStore;

/// Knowledge Base subcommands.
#[derive(Debug, Subcommand)]
pub enum KbCommands {
    /// Ingest documents for an agent's knowledge base.
    Ingest {
        /// Agent ID (e.g., "sre", "coder")
        agent: String,
        /// Force re-indexing of all documents (alias: --rebuild)
        #[arg(short, long)]
        force: bool,
        /// Ad-hoc single file to ingest (bypasses kb.toml)
        #[arg(short, long)]
        source: Option<PathBuf>,
    },
    /// List indexed documents.
    List {
        /// Agent ID (e.g., "sre", "coder"). Omit to show all collections.
        agent: Option<String>,
        /// Filter by status: indexed, failed, stale
        #[arg(short, long)]
        status: Option<String>,
    },
    /// Delete indexed documents or entire collections.
    Delete {
        /// Agent ID (alternative to --collection)
        agent: Option<String>,
        /// Collection name (alternative to --agent). E.g. "kb-sre"
        #[arg(short, long)]
        collection: Option<String>,
        /// Specific document ID to delete. If omitted, deletes all.
        #[arg(short, long)]
        doc: Option<String>,
    },
    /// Show knowledge base health for a collection.
    Status {
        /// Agent ID (e.g., "sre", "coder")
        agent: String,
    },
}

/// Run a KB command.
pub async fn run_kb_command(command: &KbCommands) -> Result<()> {
    match command {
        KbCommands::Ingest { agent, force, source } => cmd_ingest(agent, *force, source.as_ref()).await,
        KbCommands::List { agent, status } => cmd_list(agent.as_deref(), status.as_deref()).await,
        KbCommands::Delete { agent, collection, doc } => {
            cmd_delete(agent.as_deref(), collection.as_deref(), doc.as_deref()).await
        }
        KbCommands::Status { agent } => cmd_status(agent).await,
    }
}

/// Create a KnowledgeBaseManager with embedding provider and vector store.
async fn create_kb_manager() -> Result<KnowledgeBaseManager> {
    use sqlx::sqlite::SqlitePoolOptions;

    // ── Embedding provider config ──────────────────────────────────────────
    let api_key = std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("SYSCITY_EMBEDDING_API_KEY"))
        .map_err(|_| {
            crate::error::SyscityError::Validation(
                "OPENAI_API_KEY or SYSCITY_EMBEDDING_API_KEY must be set".to_string(),
            )
        })?;
    let model = std::env::var("SYSCITY_EMBEDDING_MODEL")
        .unwrap_or_else(|_| "text-embedding-3-small".to_string());
    let dimension: usize = std::env::var("SYSCITY_EMBEDDING_DIMENSION")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1536);
    let base_url = std::env::var("SYSCITY_EMBEDDING_BASE_URL").ok();

    // ── Database ───────────────────────────────────────────────────────────
    let db_path = crate::dirs::default_memory_db();
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect(&format!("sqlite://{}", db_path.display()))
        .await
        .map_err(|e| crate::error::SyscityError::Storage {
            context: "Failed to connect to database".to_string(),
            details: e.to_string(),
        })?;

    // ── Embedding provider ────────────────────────────────────────────────
    let mut provider = crate::rag::ApiEmbeddingProvider::new(
        api_key.clone(),
        model.clone(),
        dimension,
    );
    if let Some(url) = &base_url {
        provider = provider.with_base_url(url.clone());
    }
    let provider_arc: Arc<dyn EmbeddingProvider> = Arc::new(provider);
    let embedding_provider: Arc<dyn EmbeddingProvider> = Arc::new(
        crate::rag::CachedEmbeddingProvider::new(provider_arc, 1024),
    );

    // ── Vector store ──────────────────────────────────────────────────────
    let vec_store: Arc<dyn VectorStore> = Arc::new(
        crate::rag::SqliteVecStore::new(
            &format!("sqlite://{}", db_path.display()),
            dimension,
        )
        .await?,
    );

    let config = crate::rag::EmbeddingConfig {
        model,
        chunk_size: 512,
        chunk_overlap: 50,
        batch_size: 32,
        chunk_strategy: Default::default(),
    };

    Ok(KnowledgeBaseManager::new(
        embedding_provider,
        vec_store,
        pool,
        &config,
    ))
}

/// Handle `kb ingest` command.
async fn cmd_ingest(agent: &str, force: bool, source: Option<&PathBuf>) -> Result<()> {
    let manager = create_kb_manager().await?;

    let report = if let Some(source_path) = source {
        // Ad-hoc single file — bypass kb.toml
        let collection = KnowledgeBaseManager::collection_name(agent);
        let agent_dir = crate::dirs::agent_dir(agent);
        let kb_source = KnowledgeSource {
            id: None,
            name: source_path.to_string_lossy().to_string(),
            source_type: SourceType::File {
                path: source_path.to_string_lossy().to_string(),
            },
            pattern: None,
            collection: None,
            chunk_strategy: None,
        };
        manager.ingest_source(&kb_source, &collection, &agent_dir, force).await
    } else {
        manager.ingest_agent(agent, force).await
    };

    println!("Knowledge Base ingestion report for '{}':", agent);
    println!("  Sources:       {}", report.total_sources);
    println!("  Docs found:    {}", report.docs_found);
    println!("  Docs indexed:  {}", report.docs_indexed);
    println!("  Docs skipped:  {}", report.docs_skipped);
    println!("  Total chunks:  {}", report.total_chunks);
    println!("  Duration:      {:?}", report.duration);

    if !report.errors.is_empty() {
        println!("\n  Errors ({}):", report.errors.len());
        for err in &report.errors {
            println!("    - {}", err);
        }
    }

    Ok(())
}

/// Handle `kb list` command.
async fn cmd_list(agent: Option<&str>, status: Option<&str>) -> Result<()> {
    let manager = create_kb_manager().await?;

    use crate::rag::ingestion::IngestionStatus;
    let filter = status.map(|s| match s.to_lowercase().as_str() {
        "failed" => IngestionStatus::Failed,
        "stale" => IngestionStatus::Stale,
        _ => IngestionStatus::Indexed,
    });

    if let Some(agent_id) = agent {
        // Single agent/collection
        let collection = KnowledgeBaseManager::collection_name(agent_id);
        let records = manager.list(Some(&collection), filter).await?;
        let stats = manager.stats(&collection).await?;

        println!("Collection: {} ({})", collection, agent_id);
        println!("  Total docs:     {}", stats.total_docs);
        println!("  Total chunks:   {}", stats.total_chunks);
        if let Some(ref last) = stats.last_indexed_at {
            println!("  Last indexed:   {}", last);
        }
        println!("  Stale:          {}", stats.stale_count);
        println!("  Failed:         {}", stats.failed_count);
        println!();

        if records.is_empty() {
            println!("  No documents found.");
        } else {
            println!("  Documents:");
            for rec in &records {
                let status_str = match rec.status {
                    IngestionStatus::Indexed => "OK",
                    IngestionStatus::Failed => "FAIL",
                    IngestionStatus::Stale => "STALE",
                };
                println!(
                    "    [{:5}] {} ({} chunks, {})",
                    status_str, rec.doc_id, rec.chunk_count, rec.indexed_at
                );
            }
        }
    } else {
        // List all collections
        let collections = manager.list_collections().await?;
        if collections.is_empty() {
            println!("  No collections found.");
        } else {
            println!("  {:<20} {:>6} {:>8} {:>6} {:>6}  Last Indexed",
                "Collection", "Docs", "Chunks", "Stale", "Failed");
            println!("  {}", "-".repeat(70));
            for c in &collections {
                let last = c.last_indexed_at.as_deref().unwrap_or("-");
                println!(
                    "  {:<20} {:>6} {:>8} {:>6} {:>6}  {}",
                    c.collection, c.total_docs, c.total_chunks,
                    c.stale_count, c.failed_count, last
                );
            }
        }
    }

    Ok(())
}

/// Handle `kb delete` command.
async fn cmd_delete(
    agent: Option<&str>,
    collection: Option<&str>,
    doc: Option<&str>,
) -> Result<()> {
    let collection = match collection {
        Some(c) => c.to_string(),
        None => {
            let agent = agent.ok_or_else(|| {
                crate::error::SyscityError::Validation(
                    "Either --agent or --collection must be provided".to_string(),
                )
            })?;
            KnowledgeBaseManager::collection_name(agent)
        }
    };

    let manager = create_kb_manager().await?;
    let report = manager.delete(&collection, doc).await?;
    println!(
        "Deleted {} chunks from '{}'",
        report.chunks_deleted, report.collection
    );
    if let Some(did) = &report.doc_id {
        println!("  Document: {}", did);
    }

    Ok(())
}

/// Handle `kb status` command.
async fn cmd_status(agent: &str) -> Result<()> {
    let manager = create_kb_manager().await?;
    let collection = KnowledgeBaseManager::collection_name(agent);
    let stats = manager.stats(&collection).await?;

    println!("Knowledge Base status for '{}':", agent);
    println!("  Collection:     {}", collection);
    println!("  Total docs:     {}", stats.total_docs);
    println!("  Total chunks:   {}", stats.total_chunks);
    if let Some(ref last) = stats.last_indexed_at {
        println!("  Last indexed:   {}", last);
    }
    println!("  Stale:          {}", stats.stale_count);
    println!("  Failed:         {}", stats.failed_count);

    if stats.stale_count > 0 || stats.failed_count > 0 {
        println!("\n  Issues found: {}", stats.stale_count + stats.failed_count);
        if stats.stale_count > 0 {
            println!("    - {} stale document(s) — re-ingest recommended", stats.stale_count);
        }
        if stats.failed_count > 0 {
            println!("    - {} failed document(s)", stats.failed_count);
        }
    } else {
        println!("\n  All documents indexed successfully.");
    }

    Ok(())
}
