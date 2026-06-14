use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::adapters::{FileStorage, InMemoryStorage, SqliteStorage, Storage};
use crate::agent::session_store::SessionStore;
use crate::error::SyscityError;
use crate::gateway::GatewayConfig;
use crate::memory::VectorStore;
use crate::security::persistent_audit::PersistentAuditLog;
use crate::security::runtime_audit::AuditLogger;

/// Storage initialization result.
pub struct StorageInit {
    pub storage: Arc<RwLock<dyn Storage>>,
    pub unified_vector_store: Option<Arc<dyn VectorStore>>,
    pub sqlite_pool: Option<sqlx::SqlitePool>,
    pub session_store: Option<Arc<SessionStore>>,
    pub audit_log: Arc<PersistentAuditLog>,
    pub audit_log_dyn: Arc<dyn AuditLogger>,
}

/// Initialize the storage adapter, shared SQLite pool, session store, and audit log.
pub async fn init_storage(config: &GatewayConfig) -> crate::Result<StorageInit> {
    #[allow(clippy::type_complexity)]
    let (storage, unified_vector_store, sqlite_pool): (
        Arc<RwLock<dyn Storage>>,
        Option<Arc<dyn VectorStore>>,
        Option<sqlx::SqlitePool>,
    ) = match config.storage.storage_type.as_str() {
        "sqlite" => {
            let db_path = config
                .storage
                .database_url
                .as_ref()
                .map(|s| std::path::PathBuf::from(s.strip_prefix("sqlite:").unwrap_or(s)))
                .unwrap_or_else(|| crate::dirs::syscity_dir().join("data").join("syscity.db"));
            if let Some(parent) = db_path.parent() {
                tokio::fs::create_dir_all(parent).await.ok();
            }
            if !db_path.exists() {
                tokio::fs::File::create(&db_path).await.ok();
            }
            let db_url = format!("sqlite:///{}", db_path.display());
            info!("Connecting to SQLite storage at: {}", db_url);
            let pool =
                sqlx::SqlitePool::connect(&db_url)
                    .await
                    .map_err(|e| SyscityError::Storage {
                        context: "Failed to connect to SQLite".into(),
                        details: e.to_string(),
                    })?;
            let sqlite_storage = Arc::new(SqliteStorage::new(pool.clone()));
            let vector_store: Arc<dyn VectorStore> = sqlite_storage.clone();
            let storage: Arc<RwLock<dyn Storage>> =
                Arc::new(RwLock::new(SqliteStorage::new(pool.clone())));
            (storage, Some(vector_store), Some(pool))
        }
        "file" => {
            let base_path = config.storage.base_path.as_deref().unwrap_or("./data");
            let storage = Arc::new(RwLock::new(FileStorage::new(base_path)?));
            (storage, None, None)
        }
        _ => {
            let storage = Arc::new(RwLock::new(InMemoryStorage::new()));
            (storage, None, None)
        }
    };

    let session_store: Option<Arc<SessionStore>> = if let Some(ref pool) = sqlite_pool {
        match SessionStore::from_pool(pool.clone()).await {
            Ok(store) => {
                info!("SessionStore initialized for persistent chat history");
                Some(Arc::new(store))
            }
            Err(e) => {
                warn!("Failed to initialize SessionStore: {}. Chat history will not persist.", e);
                None
            }
        }
    } else {
        None
    };

    let audit_log: Arc<PersistentAuditLog> = if let Some(ref pool) = sqlite_pool {
        Arc::new(PersistentAuditLog::with_pool(pool.clone()))
    } else {
        Arc::new(PersistentAuditLog::new())
    };
    let audit_log_dyn: Arc<dyn AuditLogger> = audit_log.clone();

    Ok(StorageInit {
        storage,
        unified_vector_store,
        sqlite_pool,
        session_store,
        audit_log,
        audit_log_dyn,
    })
}
