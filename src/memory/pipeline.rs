//! Async embedding pipeline with batched background processing
//!
//! Collects embedding jobs into a bounded mpsc channel, batches them up to
//! 32 items or 100 ms (whichever comes first), then processes the batch in
//! a single `embed_batch()` call.  This amortises API latency and keeps the
//! hot path non-blocking.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info};

/// Shared state for tracking pipeline metrics.
#[derive(Debug, Default)]
#[allow(dead_code)]
struct PipelineMetrics {
    batch_count: AtomicUsize,
}

/// An embedding request sent to the pipeline.
#[derive(Debug)]
pub struct EmbeddingJob {
    /// Text to embed
    pub text: String,
    /// Optional source tag (e.g., "query", "memory", "compaction")
    pub source: String,
    /// Channel to return the result
    pub response_tx: oneshot::Sender<Result<Vec<f32>, String>>,
}

/// Configuration for the embedding pipeline.
#[derive(Debug, Clone)]
pub struct EmbeddingPipelineConfig {
    /// Maximum items per batch
    pub max_batch_size: usize,
    /// Maximum time to wait before flushing a partial batch (ms)
    pub max_wait_ms: u64,
    /// Channel capacity
    pub channel_capacity: usize,
}

impl Default for EmbeddingPipelineConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 32,
            max_wait_ms: 100,
            channel_capacity: 256,
        }
    }
}

/// Sender handle to the embedding pipeline.
#[derive(Debug, Clone)]
pub struct EmbeddingPipelineHandle {
    tx: mpsc::Sender<EmbeddingJob>,
}

impl EmbeddingPipelineHandle {
    /// Submit a single embedding job and await its result.
    pub async fn embed(&self, text: impl Into<String>) -> Result<Vec<f32>, String> {
        let (response_tx, response_rx) = oneshot::channel();
        let job = EmbeddingJob {
            text: text.into(),
            source: "query".to_string(),
            response_tx,
        };

        self.tx
            .send(job)
            .await
            .map_err(|_| "Embedding pipeline closed".to_string())?;

        response_rx
            .await
            .map_err(|_| "Embedding result channel closed".to_string())
            .and_then(|r| r)
    }

    /// Submit a job with a specific source tag.
    pub async fn embed_with_source(
        &self,
        text: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<Vec<f32>, String> {
        let (response_tx, response_rx) = oneshot::channel();
        let job = EmbeddingJob {
            text: text.into(),
            source: source.into(),
            response_tx,
        };

        self.tx
            .send(job)
            .await
            .map_err(|_| "Embedding pipeline closed".to_string())?;

        response_rx
            .await
            .map_err(|_| "Embedding result channel closed".to_string())
            .and_then(|r| r)
    }

    /// Submit a job without waiting for the result (fire-and-forget).
    /// Useful for background indexing where you don't need the embedding back.
    pub async fn submit(&self, text: impl Into<String>, source: impl Into<String>) {
        let (response_tx, _) = oneshot::channel();
        let job = EmbeddingJob {
            text: text.into(),
            source: source.into(),
            response_tx,
        };

        let _ = self.tx.send(job).await;
    }
}

/// Trait for embedding providers that can be used with the pipeline.
#[async_trait::async_trait]
pub trait PipelineEmbeddingProvider: Send + Sync {
    /// Embed a batch of texts. Returns vectors in the same order as inputs.
    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String>;
}

/// The embedding pipeline worker.
#[allow(dead_code)]
pub struct EmbeddingPipeline {
    config: EmbeddingPipelineConfig,
    handle: EmbeddingPipelineHandle,
}

impl EmbeddingPipeline {
    /// Create a new pipeline with the given provider.
    pub fn new<P>(
        provider: Arc<P>,
        config: EmbeddingPipelineConfig,
    ) -> (Self, tokio::task::JoinHandle<()>, EmbeddingPipelineHandle)
    where
        P: PipelineEmbeddingProvider + 'static,
    {
        let (tx, mut rx) = mpsc::channel::<EmbeddingJob>(config.channel_capacity);
        let handle = EmbeddingPipelineHandle { tx: tx.clone() };

        let worker_handle = tokio::spawn(async move {
            info!("Embedding pipeline started");
            let mut batch: Vec<EmbeddingJob> = Vec::with_capacity(config.max_batch_size);
            let mut deadline = tokio::time::Instant::now()
                + tokio::time::Duration::from_millis(config.max_wait_ms);

            loop {
                let timeout = tokio::time::sleep_until(deadline);
                tokio::pin!(timeout);

                tokio::select! {
                    Some(job) = rx.recv() => {
                        batch.push(job);
                        if batch.len() >= config.max_batch_size {
                            process_batch(&provider, std::mem::take(&mut batch)).await;
                            deadline = tokio::time::Instant::now()
                                + tokio::time::Duration::from_millis(config.max_wait_ms);
                            batch.reserve(config.max_batch_size);
                        }
                    }
                    _ = &mut timeout => {
                        if !batch.is_empty() {
                            process_batch(&provider, std::mem::take(&mut batch)).await;
                            batch.reserve(config.max_batch_size);
                        }
                        deadline = tokio::time::Instant::now()
                            + tokio::time::Duration::from_millis(config.max_wait_ms);
                    }
                    else => {
                        // Channel closed
                        if !batch.is_empty() {
                            process_batch(&provider, batch).await;
                        }
                        info!("Embedding pipeline shutting down");
                        break;
                    }
                }
            }
        });

        let pipeline = Self { config, handle: handle.clone() };
        (pipeline, worker_handle, handle)
    }

    /// Get a clone of the handle for submitting jobs.
    pub fn handle(&self) -> EmbeddingPipelineHandle {
        self.handle.clone()
    }
}

/// Process a batch of embedding jobs.
async fn process_batch<P: PipelineEmbeddingProvider>(provider: &Arc<P>, batch: Vec<EmbeddingJob>) {
    if batch.is_empty() {
        return;
    }

    debug!("Processing embedding batch of size {}", batch.len());

    let texts: Vec<String> = batch.iter().map(|j| j.text.clone()).collect();

    match provider.embed_batch(texts).await {
        Ok(embeddings) => {
            for (job, emb) in batch.into_iter().zip(embeddings.into_iter()) {
                let _ = job.response_tx.send(Ok(emb));
            }
        }
        Err(e) => {
            error!("Embedding batch failed: {}", e);
            for job in batch {
                let _ = job.response_tx.send(Err(e.clone()));
            }
        }
    }
}

// =============================================================================
// Default provider implementation for existing EmbeddingProvider
// =============================================================================

use super::vector::EmbeddingProvider;

#[async_trait::async_trait]
impl PipelineEmbeddingProvider for dyn EmbeddingProvider {
    async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        // Fall back to sequential embedding if the provider doesn't support batch
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            match self.embed(&text).await {
                Ok(emb) => results.push(emb),
                Err(e) => return Err(format!("Embedding failed: {}", e)),
            }
        }
        Ok(results)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockProvider {
        batch_count: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl PipelineEmbeddingProvider for MockProvider {
        async fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
            self.batch_count.fetch_add(1, Ordering::SeqCst);
            // Return dummy embeddings (same dimension as text len for testability)
            Ok(texts
                .into_iter()
                .map(|t| vec![t.len() as f32; 128])
                .collect())
        }
    }

    struct FailingMockProvider;

    #[async_trait::async_trait]
    impl PipelineEmbeddingProvider for FailingMockProvider {
        async fn embed_batch(&self, _texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
            Err("embedding failed".to_string())
        }
    }

    #[test]
    fn test_pipeline_config_default() {
        let config = EmbeddingPipelineConfig::default();
        assert_eq!(config.max_batch_size, 32);
        assert_eq!(config.max_wait_ms, 100);
        assert_eq!(config.channel_capacity, 256);
    }

    #[test]
    fn test_embedding_job_debug() {
        let (tx, _rx) = oneshot::channel();
        let job = EmbeddingJob {
            text: "hello".to_string(),
            source: "query".to_string(),
            response_tx: tx,
        };
        let debug = format!("{:?}", job);
        assert!(debug.contains("EmbeddingJob"));
    }

    #[tokio::test]
    async fn test_pipeline_batches_requests() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig {
            max_batch_size: 5,
            max_wait_ms: 1000, // Long timeout so we force batching by size
            channel_capacity: 100,
        };

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider.clone(), config);

        // Send 8 requests
        let mut handles = vec![];
        for i in 0..8 {
            let h = handle.clone();
            handles
                .push(tokio::spawn(async move { h.embed(format!("text {}", i)).await.unwrap() }));
        }

        // Wait for all
        for h in handles {
            let _ = h.await.unwrap();
        }

        // Should have processed in 2 batches (5 + 3)
        let batches = provider.batch_count.load(Ordering::SeqCst);
        assert_eq!(batches, 2);
    }

    #[tokio::test]
    async fn test_pipeline_timeout_flush() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig {
            max_batch_size: 100, // Large batch size
            max_wait_ms: 50,     // Short timeout
            channel_capacity: 100,
        };

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider.clone(), config);

        // Send just 1 request and wait longer than timeout
        let _ = handle.embed("hello").await.unwrap();

        // Wait to ensure timeout-based flush occurs
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Send second request - this will trigger a new batch
        let _ = handle.embed("world").await.unwrap();

        // Both requests processed, potentially in 1 or 2 batches depending on timing
        let batches = provider.batch_count.load(Ordering::SeqCst);
        assert!(batches >= 1, "Expected at least 1 batch, got {}", batches);
    }

    #[tokio::test]
    async fn test_embed_with_source() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig::default();

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider.clone(), config);

        let result = handle
            .embed_with_source("test", "compaction")
            .await
            .unwrap();
        assert_eq!(result.len(), 128);
    }

    #[tokio::test]
    async fn test_embed_basic() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig {
            max_batch_size: 10,
            max_wait_ms: 1000,
            channel_capacity: 100,
        };

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider.clone(), config);
        let result = handle.embed("hello world").await.unwrap();
        assert_eq!(result.len(), 128);
        // "hello world" has len 11, so all values should be 11.0
        assert!(result.iter().all(|&v| v == 11.0));
    }

    #[tokio::test]
    async fn test_embed_provider_error() {
        let provider = Arc::new(FailingMockProvider);
        let config = EmbeddingPipelineConfig {
            max_batch_size: 10,
            max_wait_ms: 50,
            channel_capacity: 100,
        };

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider, config);
        let result = handle.embed("test").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("embedding failed"));
    }

    #[tokio::test]
    async fn test_submit_fire_and_forget() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig {
            max_batch_size: 10,
            max_wait_ms: 50,
            channel_capacity: 100,
        };

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider.clone(), config);
        handle.submit("index this", "background").await;

        // Wait for timeout flush
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert_eq!(provider.batch_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_pipeline_handle_clone() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig::default();

        let (pipeline, _worker, handle) = EmbeddingPipeline::new(provider, config);
        let handle2 = pipeline.handle();
        let handle3 = handle.clone();

        // All handles should work
        let _ = handle2.embed("via pipeline").await.unwrap();
        let _ = handle3.embed("via clone").await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_embeds_same_batch() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig {
            max_batch_size: 10,
            max_wait_ms: 1000,
            channel_capacity: 100,
        };

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider.clone(), config);

        // Send 3 requests concurrently
        let r1 = handle.embed("a");
        let r2 = handle.embed("bb");
        let r3 = handle.embed("ccc");

        let (res1, res2, res3) = tokio::join!(r1, r2, r3);

        assert_eq!(res1.unwrap().len(), 128);
        assert_eq!(res2.unwrap().len(), 128);
        assert_eq!(res3.unwrap().len(), 128);
        // All 3 should fit in a single batch
        assert_eq!(provider.batch_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_embed_returns_correct_dimensions() {
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        let config = EmbeddingPipelineConfig {
            max_batch_size: 10,
            max_wait_ms: 50,
            channel_capacity: 100,
        };

        let (_pipeline, _worker, handle) = EmbeddingPipeline::new(provider, config);
        let result = handle.embed("x").await.unwrap();
        assert_eq!(result.len(), 128);
        // "x" has len 1, so all values should be 1.0
        assert!(result.iter().all(|&v| v == 1.0));
    }

    #[tokio::test]
    async fn test_empty_batch_no_panic() {
        // process_batch with empty should be a no-op
        let provider = Arc::new(MockProvider {
            batch_count: AtomicUsize::new(0),
        });
        process_batch(&provider, vec![]).await;
        assert_eq!(provider.batch_count.load(Ordering::SeqCst), 0);
    }
}
