//! Centralized registry for gateway background tasks.
//!
//! Replaces the ad-hoc `Mutex<Vec<JoinHandle<()>>>` trackers in [`Gateway`]
//! with named tasks. Tasks are stored either as ownable [`JoinHandle`]s (for
//! workers that need graceful await at shutdown) or as [`AbortHandle`]s (for
//! fire-and-forget loops whose ownership remains with a subsystem struct).

use std::collections::HashMap;

use tokio::sync::RwLock;
use tokio::task::{AbortHandle, JoinHandle};
use tracing::debug;

/// A tracked background task.
pub enum Task {
    /// A task whose [`JoinHandle`] is owned by the registry. Used for workers
    /// that shutdown needs to await with a timeout.
    Join(JoinHandle<()>),
    /// A cloneable abort handle for a task whose [`JoinHandle`] is kept
    /// elsewhere (e.g. on `DeviceInit`, `PerceptionInit`, `ControlInit`).
    Abort(AbortHandle),
}

impl Task {
    /// Abort this task, consuming the variant in the process.
    pub fn abort(self) {
        match self {
            Task::Join(handle) => handle.abort(),
            Task::Abort(handle) => handle.abort(),
        }
    }
}

/// Registry of named background tasks.
#[derive(Default)]
pub struct TaskRegistry {
    tasks: RwLock<HashMap<String, Task>>,
}

impl TaskRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Insert a [`JoinHandle`]-owned task.
    ///
    /// If a task with the same name already exists, the old task is aborted
    /// and replaced.
    pub async fn insert_join(&self, name: impl Into<String>, handle: JoinHandle<()>) {
        let name = name.into();
        let mut tasks = self.tasks.write().await;
        if let Some(old) = tasks.remove(&name) {
            debug!("Aborting previous task '{}' before replacement", name);
            old.abort();
        }
        tasks.insert(name, Task::Join(handle));
    }

    /// Register the [`AbortHandle`] of a task whose [`JoinHandle`] is kept
    /// elsewhere.
    pub async fn insert_abort(&self,
        name: impl Into<String>,
        handle: &JoinHandle<()>,
    ) {
        let name = name.into();
        let mut tasks = self.tasks.write().await;
        if let Some(old) = tasks.remove(&name) {
            debug!("Aborting previous task '{}' before replacement", name);
            old.abort();
        }
        tasks.insert(name, Task::Abort(handle.abort_handle()));
    }

    /// Abort and remove a single task by name.
    pub async fn abort(&self, name: &str) {
        if let Some(task) = self.tasks.write().await.remove(name) {
            task.abort();
        }
    }

    /// Abort and remove all tasks whose names start with `prefix`.
    pub async fn abort_matching(&self, prefix: &str) {
        let mut tasks = self.tasks.write().await;
        let keys: Vec<String> = tasks
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        for k in keys {
            if let Some(task) = tasks.remove(&k) {
                task.abort();
            }
        }
    }

    /// Abort and remove all tasks.
    pub async fn abort_all(&self) {
        let mut tasks = self.tasks.write().await;
        for (_, task) in tasks.drain() {
            task.abort();
        }
    }

    /// Remove and return a [`JoinHandle`] task by name, if present.
    ///
    /// Returns `None` if the name is missing or stored as an [`AbortHandle`].
    pub async fn take_join(&self, name: &str,
    ) -> Option<JoinHandle<()>> {
        match self.tasks.write().await.remove(name) {
            Some(Task::Join(handle)) => Some(handle),
            Some(task @ Task::Abort(_)) => {
                // Re-insert abort-handle tasks; we can't take ownership of the
                // underlying JoinHandle.
                task.abort();
                None
            }
            None => None,
        }
    }

    /// Remove and return all [`JoinHandle`] tasks whose names start with
    /// `prefix`.
    pub async fn take_matching_join(
        &self,
        prefix: &str,
    ) -> Vec<JoinHandle<()>> {
        let mut tasks = self.tasks.write().await;
        let keys: Vec<String> = tasks
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|k| match tasks.remove(&k) {
                Some(Task::Join(handle)) => Some(handle),
                Some(task) => {
                    task.abort();
                    None
                }
                None => None,
            })
            .collect()
    }

    /// Remove and return all remaining tasks.
    pub async fn take_all(&self,
    ) -> Vec<(String, Task)> {
        let mut tasks = self.tasks.write().await;
        std::mem::take(&mut *tasks).into_iter().collect()
    }
}
