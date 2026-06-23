//! Late service initialization.
//!
//! Initializes services that depend on storage or other early subsystems:
//! vector memory, session search (FTS5), hot reload, cron scheduler, task
//! scheduler, and side-effect context wiring.

use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::gateway::GatewayConfig;
use crate::gateway::GatewayEvent;
use crate::gateway::GatewayState;
use crate::memory::vector::{
    ApiEmbeddingProvider, CachedEmbeddingProvider, EmbeddingConfig, LocalGgufEmbeddingProvider,
    MemoryVectorStore, VectorMemoryService,
};
use crate::memory::{MemoryManager, SessionSearch};

/// Initialize vector memory, session search, and memory manager.
pub async fn init_memory_services(
    config: &GatewayConfig,
    state: &Arc<GatewayState>,
    sqlite_pool: Option<&sqlx::SqlitePool>,
    unified_vector_store: Option<Arc<dyn crate::memory::VectorStore>>,
) -> crate::Result<()> {
    if config.vector_memory.enabled {
        info!("Initializing vector memory service...");

        let embedding_provider: Option<Arc<dyn crate::memory::vector::EmbeddingProvider>> =
            match config.vector_memory.provider {
                crate::gateway::EmbeddingProviderType::OpenAi => {
                    if let Some(ref api_key) = config.vector_memory.embedding_api_key {
                        info!("Using OpenAI embedding provider");
                        let mut provider = ApiEmbeddingProvider::new(
                            api_key.clone(),
                            config.vector_memory.embedding_model.clone(),
                            config.vector_memory.embedding_dimension,
                        );
                        if let Some(ref base_url) = config.vector_memory.api_base_url {
                            provider = provider.with_base_url(base_url.clone());
                        }
                        Some(Arc::new(provider))
                    } else {
                        warn!("OpenAI embedding provider requires an API key");
                        None
                    }
                }
                crate::gateway::EmbeddingProviderType::LocalGguf => {
                    #[cfg(feature = "local-embeddings")]
                    {
                        if let Some(ref model_path) = config.vector_memory.local_model_path {
                            info!("Using local GGUF embedding provider");
                            use crate::memory::local_embeddings::ModelSource;
                            let source = ModelSource::parse(model_path);
                            let provider = LocalGgufEmbeddingProvider::create(
                                source,
                                config.vector_memory.embedding_dimension,
                            )
                            .await;
                            if provider.is_fts_only() {
                                if let Some(reason) = provider.fts_reason() {
                                    warn!("Local GGUF provider in FTS-only mode: {}", reason);
                                } else {
                                    info!(
                                        "Local GGUF provider initialized, will load model on \
                                         first use"
                                    );
                                }
                            } else {
                                info!("GGUF model configured from {}", model_path);
                            }
                            Some(Arc::new(provider))
                        } else {
                            warn!("Local GGUF provider requires 'local_model_path' configuration");
                            None
                        }
                    }
                    #[cfg(not(feature = "local-embeddings"))]
                    {
                        warn!(
                            "Local GGUF provider requires 'local-embeddings' feature. Build with: \
                             cargo build --features local-embeddings"
                        );
                        None
                    }
                }
            };

        if let Some(embedding_provider) = embedding_provider {
            let vector_store: Arc<dyn crate::memory::VectorStore> = match unified_vector_store {
                Some(store) => {
                    info!("Using unified SQLite storage for vector store");
                    store
                }
                None => {
                    info!(
                        "Using in-memory vector store (unified storage requires 'sqlite' storage \
                         type)"
                    );
                    Arc::new(MemoryVectorStore::new(config.vector_memory.embedding_dimension))
                }
            };

            let embedding_config = EmbeddingConfig {
                model: config.vector_memory.embedding_model.clone(),
                chunk_size: 512,
                chunk_overlap: 50,
                batch_size: 32,
            };

            let cached_provider = CachedEmbeddingProvider::new(embedding_provider, 1024);
            let service = Arc::new(VectorMemoryService::new(
                Arc::new(cached_provider),
                vector_store,
                &embedding_config,
            ));
            info!(
                "Vector memory service initialized with {:?} provider",
                config.vector_memory.provider
            );
            *state.memory.vector.write().await = Some(service);
        } else {
            warn!("Vector memory enabled but no suitable provider available");
        }
    } else {
        info!("Vector memory service disabled");
    }

    if let Some(pool) = sqlite_pool {
        info!("Initializing session search (FTS5)...");
        let session_search = Arc::new(SessionSearch::new(pool.clone()));
        if let Err(e) = session_search.initialize().await {
            warn!("Failed to initialize session search: {}", e);
        } else {
            info!("Session search (FTS5) initialized");
            let session_search_for_mm = session_search.clone();
            *state.memory.session_search.write().await = Some(session_search.clone());

            if let Some(vector_svc) = state.memory.vector.read().await.clone() {
                info!("Initializing MemoryManager with hybrid search...");
                let store = Arc::new(
                    crate::memory::UnifiedStore::new_with_pool(pool.clone())
                        .await
                        .map_err(|e| crate::error::SyscityError::Storage {
                            context: "Failed to create UnifiedStore".into(),
                            details: e.to_string(),
                        })?,
                );
                let mm = MemoryManager::new(
                    store.clone(),
                    store,
                    crate::memory::MemoryManagerConfig::default(),
                )
                .with_vector_service(vector_svc)
                .with_session_search(session_search_for_mm);
                state.memory.manager.write().await.replace(Arc::new(mm));
                info!("MemoryManager with hybrid search initialized");
            } else {
                info!("Initializing MemoryManager (vector search disabled)...");
                let store = Arc::new(
                    crate::memory::UnifiedStore::new_with_pool(pool.clone())
                        .await
                        .map_err(|e| crate::error::SyscityError::Storage {
                            context: "Failed to create UnifiedStore".into(),
                            details: e.to_string(),
                        })?,
                );
                let mm = MemoryManager::new(
                    store.clone(),
                    store,
                    crate::memory::MemoryManagerConfig::default(),
                )
                .with_session_search(session_search_for_mm);
                state.memory.manager.write().await.replace(Arc::new(mm));
                info!("MemoryManager initialized (without vector search)");
            }
        }
    } else {
        info!("SQLite not in use; session search and hybrid memory disabled");
    }

    Ok(())
}

/// Initialize hot reload manager if enabled.
pub async fn init_hot_reload(config: &GatewayConfig, state: &Arc<GatewayState>) {
    if config.hot_reload.enabled {
        info!("Initializing hot reload manager...");
        match crate::config::hot_reload::HotReloadManager::new() {
            Ok(manager) => {
                let manager = Arc::new(manager);
                info!("Hot reload manager initialized");
                *state.infra.hot_reload.write().await = Some(manager);
            }
            Err(e) => {
                warn!("Failed to initialize hot reload manager: {}", e);
            }
        }
    } else {
        info!("Hot reload disabled");
    }
}

/// Initialize cron scheduler if enabled.
pub async fn init_cron(config: &GatewayConfig, state: &Arc<GatewayState>) {
    if config.cron.enabled {
        info!("Initializing advanced cron scheduler...");
        use crate::cron::cron::{AnnounceDelivery, CronScheduler};
        let (cron_scheduler, command_rx) = CronScheduler::new();
        let cron_scheduler =
            cron_scheduler.with_store_path(crate::dirs::cron_dir().join("jobs.json"));
        let cron_scheduler = Arc::new(Mutex::new(cron_scheduler));

        let (announce_tx, mut announce_rx) = mpsc::channel::<AnnounceDelivery>(64);
        {
            let mut scheduler = cron_scheduler.lock().await;
            scheduler.set_announce_tx(announce_tx);
        }
        let event_tx_announce = state.events.tx.clone();
        let announce_handle = tokio::spawn(async move {
            while let Some(delivery) = announce_rx.recv().await {
                info!("Cron announce → {}:{}", delivery.channel, delivery.to);
                match event_tx_announce.send(GatewayEvent::CronAnnounce {
                    channel: delivery.channel,
                    to: delivery.to,
                    message: delivery.message.clone(),
                }) {
                    Ok(receiver_count) => {
                        info!("Cron announce broadcast to {} receivers", receiver_count)
                    }
                    Err(e) => warn!("Failed to broadcast cron announce: {}", e),
                }
            }
        });
        state.task_registry.insert_join("cron:announce", announce_handle).await;

        let cron_scheduler_clone = Arc::clone(&cron_scheduler);
        let scheduler_handle = tokio::spawn(async move {
            let mut scheduler = cron_scheduler_clone.lock().await;
            if let Err(e) = scheduler.start(command_rx).await {
                warn!("Advanced cron scheduler failed: {}", e);
            }
        });
        state.task_registry.insert_join("cron:scheduler", scheduler_handle).await;
        *state
            .scheduler
            .cron_scheduler
            .write()
            .await = Some(cron_scheduler.clone());
        info!("Advanced cron scheduler initialized");

        crate::tools::CronTool::set_scheduler(cron_scheduler);
    } else {
        info!("Cron scheduler disabled");
    }
}

/// Initialize the recurring task scheduler.
pub async fn init_task_scheduler(state: &Arc<GatewayState>) {
    let mut task_scheduler = crate::planner::TaskScheduler::new();
    let state_for_scheduler = Arc::clone(state);
    let handler: crate::planner::scheduled_tasks::TaskHandler = Arc::new(move |task| {
        let state = Arc::clone(&state_for_scheduler);
        Box::pin(async move {
            let agents = state.agents.agents.read().await;
            if let Some(handle) = agents.values().next() {
                let msg = format!(
                    "[Scheduled] {}: {} - Actions: {}",
                    task.name,
                    task.description,
                    task.actions.len()
                );
                let incoming = crate::channels::IncomingMessage::new("scheduler", &task.id, &msg);
                if let Err(e) = handle.agent.process_message(incoming).await {
                    warn!("Scheduled task '{}' failed: {}", task.id, e);
                } else {
                    info!("Scheduled task '{}' processed successfully", task.id);
                }
            } else {
                warn!("No agent available to run scheduled task '{}'", task.id);
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    });
    if let Err(e) = task_scheduler.start(handler).await {
        warn!("TaskScheduler failed to start: {}", e);
    } else {
        *state
            .scheduler
            .task_scheduler
            .write()
            .await = Some(Arc::new(Mutex::new(task_scheduler)));
        info!("TaskScheduler started");
    }
}

/// Wire side-effect executor with runtime context (memory + cron).
pub async fn init_side_effect_context(state: &Arc<GatewayState>) {
    let side_effect_ctx = crate::outbound::SideEffectContext {
        memory_manager: state.memory.manager.read().await.as_ref().cloned(),
        cron_scheduler: state.scheduler.cron_scheduler.read().await.clone(),
        webhook_client: Some(Arc::new(
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
        )),
    };
    state
        .pipelines
        .side_effect_executor
        .set_context(side_effect_ctx)
        .await;
    info!("SideEffectExecutor context wired");
}

/// Convenience helper that initializes all late services in dependency order.
pub async fn init_late_services(
    config: &GatewayConfig,
    state: &Arc<GatewayState>,
    sqlite_pool: Option<&sqlx::SqlitePool>,
    unified_vector_store: Option<Arc<dyn crate::memory::VectorStore>>,
) -> crate::Result<()> {
    init_memory_services(config, state, sqlite_pool, unified_vector_store).await?;
    init_hot_reload(config, state).await;
    init_cron(config, state).await;
    init_task_scheduler(state).await;
    init_side_effect_context(state).await;
    Ok(())
}
