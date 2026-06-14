//! Lazy, single-assignment wrapper for values that are initialized after
//! [`GatewayState`](crate::gateway::GatewayState) is constructed.
//!
//! Typical use cases are subsystems that depend on the shared state (e.g. vector
//! memory, cron scheduler) and therefore cannot be created until the state
//! struct exists.

use std::sync::Arc;
use tokio::sync::RwLock;

/// A value that is initialized exactly once and read many times.
///
/// [`LateInit`] is cheaply cloneable so it can be shared between the gateway
/// state and any background tasks or tools that need to observe the value once
/// it becomes available.
#[derive(Clone)]
pub struct LateInit<T> {
    inner: Arc<RwLock<Option<T>>>,
}

impl<T> LateInit<T> {
    /// Create an uninitialized [`LateInit`].
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
        }
    }

    /// Initialize the value. Panics if already initialized.
    pub async fn init(&self, value: T) {
        let mut guard = self.inner.write().await;
        assert!(guard.is_none(), "LateInit already initialized");
        *guard = Some(value);
    }

    /// Return a clone of the initialized value.
    ///
    /// Panics if [`LateInit::init`] has not been called.
    pub async fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner
            .read()
            .await
            .clone()
            .expect("LateInit not initialized")
    }

    /// Return a clone of the value if it has been initialized.
    pub async fn get_opt(&self) -> Option<T>
    where
        T: Clone,
    {
        self.inner.read().await.clone()
    }

    /// Return `true` if the value has been initialized.
    pub async fn is_initialized(&self) -> bool {
        self.inner.read().await.is_some()
    }
}

impl<T> Default for LateInit<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_late_init_basic() {
        let init: LateInit<String> = LateInit::new();
        assert!(!init.is_initialized().await);
        init.init("hello".to_string()).await;
        assert!(init.is_initialized().await);
        assert_eq!(init.get().await, "hello");
    }

    #[tokio::test]
    async fn test_late_init_clone_shares_value() {
        let init: LateInit<Arc<i32>> = LateInit::new();
        let cloned = init.clone();
        init.init(Arc::new(42)).await;
        assert_eq!(*cloned.get().await, 42);
    }

    #[tokio::test]
    async fn test_late_init_get_opt() {
        let init: LateInit<i32> = LateInit::new();
        assert_eq!(init.get_opt().await, None);
        init.init(7).await;
        assert_eq!(init.get_opt().await, Some(7));
    }
}
