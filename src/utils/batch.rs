//! Request batching utilities for Manta
//!
//! Provides batching of requests to improve throughput and reduce overhead.

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::time::interval;
use tracing::{debug, trace, warn};

/// Batched request
#[derive(Debug)]
pub struct BatchedRequest<I, O> {
    /// Request ID
    pub id: I,
    /// Response channel
    pub response_tx: oneshot::Sender<O>,
    /// Timestamp when request was queued
    pub queued_at: Instant,
}

/// Batch processor trait
#[async_trait::async_trait]
pub trait BatchProcessor: Send + Sync {
    /// Input type
    type Input: Send + Clone;
    /// Output type
    type Output: Send + Clone;
    /// Error type
    type Error: Send + Clone + std::fmt::Debug;

    /// Process a batch of inputs
    async fn process_batch(
        &self,
        inputs: Vec<Self::Input>,
    ) -> Result<Vec<Self::Output>, Self::Error>;
}

/// Batch configuration
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum batch size
    pub max_batch_size: usize,
    /// Maximum time to wait before processing a batch
    pub max_wait: Duration,
    /// Minimum batch size to trigger processing
    pub min_batch_size: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 100,
            max_wait: Duration::from_millis(10),
            min_batch_size: 1,
        }
    }
}

impl BatchConfig {
    /// Create a new batch configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum batch size
    pub fn with_max_size(mut self, size: usize) -> Self {
        self.max_batch_size = size;
        self
    }

    /// Set maximum wait time
    pub fn with_max_wait(mut self, wait: Duration) -> Self {
        self.max_wait = wait;
        self
    }

    /// Set minimum batch size
    pub fn with_min_size(mut self, size: usize) -> Self {
        self.min_batch_size = size;
        self
    }
}

/// Batcher for grouping and processing requests in batches
#[derive(Debug)]
pub struct Batcher<I, O> {
    config: BatchConfig,
    request_tx: mpsc::Sender<BatchedRequest<I, O>>,
}

impl<I, O> Batcher<I, O>
where
    I: Send + Clone + 'static,
    O: Send + Clone + 'static,
{
    /// Create a new batcher with a processor
    pub fn new<P>(config: BatchConfig, processor: P) -> Self
    where
        P: BatchProcessor<Input = I, Output = O> + 'static,
    {
        let (request_tx, request_rx) = mpsc::channel(config.max_batch_size * 2);

        tokio::spawn(run_batcher(config.clone(), request_rx, processor));

        Self { config, request_tx }
    }

    /// Submit a request to be batched
    pub async fn submit(&self, id: I) -> Result<O, BatchError> {
        let (response_tx, response_rx) = oneshot::channel();

        let request = BatchedRequest {
            id,
            response_tx,
            queued_at: Instant::now(),
        };

        self.request_tx
            .send(request)
            .await
            .map_err(|_| BatchError::BatcherClosed)?;

        response_rx.await.map_err(|_| BatchError::ResponseClosed)
    }

    /// Get batch configuration
    pub fn config(&self) -> &BatchConfig {
        &self.config
    }
}

impl<I, O> Clone for Batcher<I, O> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            request_tx: self.request_tx.clone(),
        }
    }
}

/// Batch error
#[derive(Debug, Clone)]
pub enum BatchError {
    /// Batcher has been closed
    BatcherClosed,
    /// Response channel closed
    ResponseClosed,
    /// Processing error
    Processing(String),
}

impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BatchError::BatcherClosed => write!(f, "Batcher has been closed"),
            BatchError::ResponseClosed => write!(f, "Response channel closed"),
            BatchError::Processing(msg) => write!(f, "Processing error: {}", msg),
        }
    }
}

impl std::error::Error for BatchError {}

/// Run the batch processing loop
async fn run_batcher<I, O, P>(
    config: BatchConfig,
    mut request_rx: mpsc::Receiver<BatchedRequest<I, O>>,
    processor: P,
) where
    I: Send + Clone,
    O: Send + Clone,
    P: BatchProcessor<Input = I, Output = O>,
{
    let mut buffer: Vec<BatchedRequest<I, O>> = Vec::with_capacity(config.max_batch_size);
    let mut ticker = interval(config.max_wait);

    loop {
        tokio::select! {
            // Receive new requests
            Some(request) = request_rx.recv() => {
                trace!("Received request for batching");
                buffer.push(request);

                // Process if buffer is full
                if buffer.len() >= config.max_batch_size {
                    let batch = std::mem::take(&mut buffer);
                    process_buffer(batch, &processor).await;
                }
            }

            // Process on timeout if we have enough items
            _ = ticker.tick() => {
                if buffer.len() >= config.min_batch_size {
                    trace!("Processing batch due to timeout");
                    let batch = std::mem::take(&mut buffer);
                    process_buffer(batch, &processor).await;
                }
            }

            // Channel closed
            else => {
                debug!("Batcher input channel closed");
                // Process remaining items
                if !buffer.is_empty() {
                    let batch = std::mem::take(&mut buffer);
                    process_buffer(batch, &processor).await;
                }
                break;
            }
        }
    }
}

/// Process a buffer of requests
async fn process_buffer<I, O, P>(buffer: Vec<BatchedRequest<I, O>>, processor: &P)
where
    I: Clone + Send,
    O: Clone + Send,
    P: BatchProcessor<Input = I, Output = O>,
{
    if buffer.is_empty() {
        return;
    }

    let inputs: Vec<I> = buffer.iter().map(|r| r.id.clone()).collect();
    let start = Instant::now();

    match processor.process_batch(inputs).await {
        Ok(outputs) => {
            let elapsed = start.elapsed();
            debug!(
                batch_size = buffer.len(),
                elapsed_ms = elapsed.as_millis() as u64,
                "Batch processed successfully"
            );

            // Send responses
            for (request, output) in buffer.into_iter().zip(outputs.into_iter()) {
                let _ = request.response_tx.send(output);
            }
        }
        Err(e) => {
            warn!("Batch processing failed: {:?}", e);
            // Note: We can't send the error through the output channel
            // since O is the expected output type. The requests will
            // be dropped and callers will receive a ResponseClosed error.
        }
    }
}

/// Simple batch processor for function-based batching
pub struct FunctionBatchProcessor<I, O> {
    f: Box<dyn Fn(Vec<I>) -> Vec<O> + Send + Sync>,
}

impl<I, O> FunctionBatchProcessor<I, O> {
    /// Create a new processor from a function
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(Vec<I>) -> Vec<O> + Send + Sync + 'static,
    {
        Self { f: Box::new(f) }
    }
}

#[async_trait::async_trait]
impl<I, O> BatchProcessor for FunctionBatchProcessor<I, O>
where
    I: Send + Clone + 'static,
    O: Send + Clone + 'static,
{
    type Input = I;
    type Output = O;
    type Error = ();

    async fn process_batch(&self, inputs: Vec<I>) -> Result<Vec<O>, Self::Error> {
        Ok((self.f)(inputs))
    }
}

/// Batch statistics
#[derive(Debug, Default)]
pub struct BatchStats {
    /// Total batches processed
    pub total_batches: u64,
    /// Total items processed
    pub total_items: u64,
    /// Average batch size
    pub avg_batch_size: f64,
    /// Average processing time
    pub avg_processing_time_ms: f64,
}

/// Request deduplicator for avoiding duplicate requests
#[derive(Debug, Clone)]
pub struct Deduplicator<K, V> {
    pending: std::sync::Arc<tokio::sync::Mutex<HashMap<K, Vec<oneshot::Sender<V>>>>>,
}

impl<K, V> Deduplicator<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Create a new deduplicator
    pub fn new() -> Self {
        Self {
            pending: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Try to start a request, returns None if already pending
    pub async fn try_start(&self, key: K) -> Option<PendingRequest<K, V>> {
        let mut pending = self.pending.lock().await;

        if pending.contains_key(&key) {
            None
        } else {
            pending.insert(key.clone(), vec![]);
            Some(PendingRequest {
                key,
                pending: self.pending.clone(),
                completed: false,
            })
        }
    }

    /// Wait for an existing request to complete
    pub async fn wait_for(&self, key: K) -> Result<V, DeduplicateError> {
        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock().await;
            if let Some(waiters) = pending.get_mut(&key) {
                waiters.push(tx);
            } else {
                return Err(DeduplicateError::NotPending);
            }
        }

        rx.await.map_err(|_| DeduplicateError::SenderDropped)
    }
}

impl<K, V> Default for Deduplicator<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

/// Pending request handle
pub struct PendingRequest<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + Sync + 'static,
{
    key: K,
    pending: std::sync::Arc<tokio::sync::Mutex<HashMap<K, Vec<oneshot::Sender<V>>>>>,
    completed: bool,
}

impl<K, V> PendingRequest<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Complete the request and notify all waiters
    pub async fn complete(mut self, value: V) {
        self.completed = true;
        let mut pending = self.pending.lock().await;

        if let Some(waiters) = pending.remove(&self.key) {
            for tx in waiters {
                let _ = tx.send(value.clone());
            }
        }
    }
}

impl<K, V> Drop for PendingRequest<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if !self.completed {
            // Remove from pending if not completed
            let key = self.key.clone();
            let pending = self.pending.clone();
            tokio::spawn(async move {
                let mut pending = pending.lock().await;
                pending.remove(&key);
            });
        }
    }
}

/// Deduplication error
#[derive(Debug, Clone)]
pub enum DeduplicateError {
    /// Key not found in pending
    NotPending,
    /// Sender dropped
    SenderDropped,
}

impl std::fmt::Display for DeduplicateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeduplicateError::NotPending => write!(f, "Key not pending"),
            DeduplicateError::SenderDropped => write!(f, "Sender dropped"),
        }
    }
}

impl std::error::Error for DeduplicateError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_batcher() {
        let processor =
            FunctionBatchProcessor::new(|inputs: Vec<i32>| inputs.iter().map(|x| x * 2).collect());

        let batcher = Batcher::new(
            BatchConfig::new()
                .with_max_size(5)
                .with_max_wait(Duration::from_millis(50)),
            processor,
        );

        // Submit multiple requests
        let mut handles = Vec::new();
        for i in 0..10 {
            let batcher = batcher.clone();
            handles.push(tokio::spawn(async move { batcher.submit(i).await }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap().unwrap();
            assert_eq!(result, (i as i32) * 2);
        }
    }

    #[tokio::test]
    async fn test_deduplicator() {
        let dedup: Deduplicator<String, i32> = Deduplicator::new();

        // Start first request
        let request = dedup.try_start("key1".to_string()).await;
        assert!(request.is_some());

        // Second request should be deduplicated
        let request2 = dedup.try_start("key1".to_string()).await;
        assert!(request2.is_none());

        // Wait for result in background
        let dedup2 = dedup.clone();
        let wait_handle = tokio::spawn(async move { dedup2.wait_for("key1".to_string()).await });

        // Complete the first request
        tokio::time::sleep(Duration::from_millis(10)).await;
        request.unwrap().complete(42).await;

        // Waiter should receive the result
        let result = wait_handle.await.unwrap();
        assert_eq!(result.unwrap(), 42);
    }
}
