use tracing::info;

use super::AcpControlPlane;
use crate::acp::config::{SubagentStatus, ThreadContext, ThreadContextSummary};

impl AcpControlPlane {
    /// Ensure a thread context exists in the control plane.
    pub async fn ensure_thread(&self, thread_id: &str) {
        let mut threads = self.threads.write().await;
        if !threads.contains_key(thread_id) {
            threads.insert(
                thread_id.to_string(),
                ThreadContext {
                    id: thread_id.to_string(),
                    active_subagent: None,
                    created_at: chrono::Utc::now(),
                },
            );
        }
    }

    /// List snapshots of all known threads.
    pub async fn list_threads(&self) -> Vec<ThreadContextSummary> {
        let threads = self.threads.read().await;
        threads
            .values()
            .map(|t| ThreadContextSummary {
                id: t.id.clone(),
                active_subagent: t.active_subagent.clone(),
                created_at: t.created_at,
            })
            .collect()
    }

    /// Get a snapshot of a thread context.
    pub async fn get_thread_context(&self, thread_id: &str) -> Option<ThreadContextSummary> {
        let threads = self.threads.read().await;
        threads.get(thread_id).map(|t| ThreadContextSummary {
            id: t.id.clone(),
            active_subagent: t.active_subagent.clone(),
            created_at: t.created_at,
        })
    }

    /// Switch the active subagent on a thread.
    ///
    /// This performs a thread context switch: the given subagent becomes the
    /// active context on the thread. Passing `None` clears the active context.
    pub async fn switch_thread_active_subagent(
        &self,
        thread_id: &str,
        subagent_id: Option<&str>,
    ) -> crate::Result<()> {
        if let Some(id) = subagent_id {
            let subagents = self.subagents.read().await;
            let handle = subagents
                .get(id)
                .ok_or_else(|| crate::error::SyscityError::NotFound {
                    resource: format!("Subagent '{}'", id),
                })?;
            if handle.thread_id != thread_id {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Subagent {} is bound to thread {}, not {}",
                    id, handle.thread_id, thread_id
                )));
            }
            if handle.status == SubagentStatus::Terminated
                || handle.status == SubagentStatus::Crashed
            {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Cannot switch to {:?} subagent {}",
                    handle.status, id
                )));
            }
        }

        self.ensure_thread(thread_id).await;

        let mut threads = self.threads.write().await;
        let thread = threads.get_mut(thread_id).ok_or_else(|| {
            crate::error::SyscityError::Internal(format!("Thread {} disappeared", thread_id))
        })?;
        thread.active_subagent = subagent_id.map(|s| s.to_string());
        info!("Switched active subagent on thread {} to {:?}", thread_id, subagent_id);
        self.emit(crate::gateway::GatewayEvent::AcpThreadSwitched {
            thread_id: thread_id.to_string(),
            active_subagent: subagent_id.map(|s| s.to_string()),
        })
        .await;
        Ok(())
    }

    /// Migrate a subagent to a different thread.
    ///
    /// The subagent's `thread_id` is updated and the old thread clears its
    /// active subagent reference if it pointed to this subagent.
    pub async fn migrate_subagent_thread(
        &self,
        subagent_id: &str,
        target_thread_id: &str,
    ) -> crate::Result<()> {
        let (old_thread_id, _status) = {
            let subagents = self.subagents.read().await;
            let handle =
                subagents
                    .get(subagent_id)
                    .ok_or_else(|| crate::error::SyscityError::NotFound {
                        resource: format!("Subagent '{}'", subagent_id),
                    })?;
            if handle.status == SubagentStatus::Terminated
                || handle.status == SubagentStatus::Crashed
            {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Cannot migrate {:?} subagent {}",
                    handle.status, subagent_id
                )));
            }
            (handle.thread_id.clone(), handle.status)
        };

        if old_thread_id == target_thread_id {
            return Ok(());
        }

        self.ensure_thread(target_thread_id).await;

        {
            let mut threads = self.threads.write().await;
            if let Some(old) = threads.get_mut(&old_thread_id) {
                if old.active_subagent.as_deref() == Some(subagent_id) {
                    old.active_subagent = None;
                }
            }
            let target = threads.get_mut(target_thread_id).ok_or_else(|| {
                crate::error::SyscityError::Internal(format!(
                    "Thread {} disappeared after creation",
                    target_thread_id
                ))
            })?;
            if matches!(
                target.active_subagent.as_deref(),
                Some(id) if id != subagent_id
            ) {
                return Err(crate::error::SyscityError::Internal(format!(
                    "Thread {} already has active subagent {}",
                    target_thread_id,
                    target.active_subagent.as_deref().unwrap_or("")
                )));
            }
            target.active_subagent = Some(subagent_id.to_string());
        }

        {
            let mut subagents = self.subagents.write().await;
            if let Some(handle) = subagents.get_mut(subagent_id) {
                handle.thread_id = target_thread_id.to_string();
            }
        }

        info!(
            "Migrated subagent {} from thread {} to thread {}",
            subagent_id, old_thread_id, target_thread_id
        );
        Ok(())
    }
}
