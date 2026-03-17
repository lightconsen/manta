//! Local GGUF Embedding Implementation using llama.cpp
//!
//! This module provides embedding generation using GGUF format models
//! with HuggingFace Hub auto-download support.

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::token::LlamaToken;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tracing::{debug, info, warn};

/// HuggingFace model cache directory
const HF_CACHE_DIR: &str = ".manta/models";

/// Global backend singleton (llama.cpp requires single backend instance)
static LLAMA_BACKEND: OnceLock<crate::Result<LlamaBackend>> = OnceLock::new();

/// Get or initialize the llama.cpp backend
fn get_backend() -> crate::Result<&'static LlamaBackend> {
    let result = LLAMA_BACKEND.get_or_init(|| {
        LlamaBackend::init()
            .map_err(|e| crate::error::MantaError::Validation(format!("Failed to init llama.cpp backend: {}", e)))
    });
    match result {
        Ok(ref backend) => Ok(backend),
        Err(ref e) => Err(crate::error::MantaError::Validation(format!("Backend initialization failed: {}", e))),
    }
}

/// Model source - either local path or HuggingFace Hub reference
#[derive(Debug, Clone)]
pub enum ModelSource {
    /// Local file path
    Local(PathBuf),
    /// HuggingFace Hub model (repo_id, filename)
    HuggingFace { repo_id: String, filename: String },
}

impl ModelSource {
    /// Parse a model source string
    ///
    /// Supports:
    /// - Local paths: `/path/to/model.gguf` or `./model.gguf`
    /// - HF Hub: `hf:repo_id/filename` or `repo_id/filename`
    pub fn parse(source: &str) -> Self {
        if source.starts_with("hf:") {
            let parts = source.strip_prefix("hf:").unwrap_or(source);
            Self::parse_hf_format(parts)
        } else if source.starts_with("https://huggingface.co/") {
            // Parse full HF URL
            let path = source.strip_prefix("https://huggingface.co/").unwrap_or(source);
            Self::parse_hf_format(path)
        } else if source.contains('/') && !source.starts_with("./") && !source.starts_with("/") {
            // Likely HF format: org/model/filename.gguf
            Self::parse_hf_format(source)
        } else {
            // Local path
            ModelSource::Local(PathBuf::from(source))
        }
    }

    fn parse_hf_format(s: &str) -> Self {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() >= 2 {
            let repo_id = format!("{}/{}", parts[0], parts[1]);
            let filename = if parts.len() > 2 {
                parts[2..].join("/")
            } else {
                "model.gguf".to_string()
            };
            ModelSource::HuggingFace { repo_id, filename }
        } else {
            ModelSource::Local(PathBuf::from(s))
        }
    }

    /// Resolve to a local path, downloading if necessary
    pub async fn resolve(&self) -> crate::Result<PathBuf> {
        match self {
            ModelSource::Local(path) => Ok(path.clone()),
            ModelSource::HuggingFace { repo_id, filename } => {
                download_from_hf(repo_id, filename).await
            }
        }
    }
}

/// Download model from HuggingFace Hub
async fn download_from_hf(repo_id: &str, filename: &str) -> crate::Result<PathBuf> {
    info!("Downloading model from HuggingFace Hub: {}/{}", repo_id, filename);

    let cache_dir = dirs::home_dir()
        .map(|h| h.join(HF_CACHE_DIR))
        .unwrap_or_else(|| PathBuf::from(HF_CACHE_DIR));

    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| crate::error::MantaError::Io(e))?;

    // Use hf-hub for download
    let api = hf_hub::api::tokio::Api::new()
        .map_err(|e| crate::error::MantaError::Validation(format!("Failed to create HF API: {}", e)))?;

    let repo = api.model(repo_id.to_string());

    let local_path = repo
        .get(filename)
        .await
        .map_err(|e| crate::error::MantaError::Validation(format!("Failed to download model: {}", e)))?;

    info!("Model downloaded to: {:?}", local_path);
    Ok(local_path)
}

/// Lazy-initialized embedding model
pub struct LazyEmbeddingModel {
    source: ModelSource,
    model_name: String,
    dimension: usize,
    /// The actual model (initialized on first use)
    inner: tokio::sync::OnceCell<EmbeddingModelInner>,
}

/// Inner model struct (initialized lazily)
struct EmbeddingModelInner {
    model: LlamaModel,
    context_params: LlamaContextParams,
    backend: &'static LlamaBackend,
}

impl LazyEmbeddingModel {
    /// Create a new lazy embedding model
    pub fn new(source: ModelSource, dimension: usize) -> Self {
        let model_name = match &source {
            ModelSource::Local(path) => path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string(),
            ModelSource::HuggingFace { repo_id, filename } => {
                format!("{}/{}", repo_id, filename)
            }
        };

        Self {
            source,
            model_name,
            dimension,
            inner: tokio::sync::OnceCell::new(),
        }
    }

    /// Get or initialize the model
    async fn get_model(&self) -> crate::Result<&EmbeddingModelInner> {
        self.inner
            .get_or_try_init(|| async {
                let path = self.source.resolve().await?;
                info!("Initializing GGUF model from: {:?}", path);

                let backend = get_backend()?;

                let model_params = LlamaModelParams::default();

                let model = LlamaModel::load_from_file(
                    backend,
                    &path,
                    &model_params,
                ).map_err(|e| crate::error::MantaError::Validation(format!("Failed to load model: {}", e)))?;

                let context_params = LlamaContextParams::default()
                    .with_n_batch(512);

                info!("GGUF model loaded successfully");

                Ok(EmbeddingModelInner {
                    model,
                    context_params,
                    backend,
                })
            })
            .await
    }

    /// Generate embeddings for a batch of texts
    pub async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        let inner = self.get_model().await?;

        let mut embeddings = Vec::with_capacity(texts.len());

        for text in texts {
            let embedding = self.embed_single(inner, text).await?;
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    async fn embed_single(&self, inner: &EmbeddingModelInner, text: &str) -> crate::Result<Vec<f32>> {
        // Tokenize the input
        let tokens = self.tokenize(inner, text)?;

        // Create a context for inference
        let mut ctx = inner.model
            .new_context(inner.backend, inner.context_params.clone())
            .map_err(|e| crate::error::MantaError::Validation(format!("Failed to create context: {}", e)))?;

        // Create batch
        let mut batch = LlamaBatch::new(512, 1);

        // Add tokens to batch
        for (i, token) in tokens.iter().enumerate() {
            let is_last = i == tokens.len() - 1;
            batch.add(*token, i as i32, &[0], is_last)
                .map_err(|e| crate::error::MantaError::Validation(format!("Failed to add token: {}", e)))?;
        }

        // Decode
        ctx.decode(&mut batch)
            .map_err(|e| crate::error::MantaError::Validation(format!("Decode failed: {}", e)))?;

        // Extract embeddings from the last token
        let embedding = ctx.embeddings_seq_ith(0)
            .map_err(|e| crate::error::MantaError::Validation(format!("Failed to get embeddings: {}", e)))?;

        // Convert to Vec<f32> and normalize
        let mut vec: Vec<f32> = embedding.iter().map(|x| *x as f32).collect();

        // Normalize to unit vector
        let magnitude: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if magnitude > 0.0 {
            vec.iter_mut().for_each(|x| *x /= magnitude);
        }

        Ok(vec)
    }

    fn tokenize(&self, inner: &EmbeddingModelInner, text: &str) -> crate::Result<Vec<LlamaToken>> {
        // Simple tokenization using the model's tokenizer
        // Use add_bos=true for embeddings (helps with sentence representation)
        use llama_cpp_2::model::AddBos;
        let tokens = inner.model.str_to_token(text, AddBos::Always)
            .map_err(|e| crate::error::MantaError::Validation(format!("Tokenization failed: {}", e)))?;

        Ok(tokens)
    }

    /// Get model name
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Get embedding dimension
    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// Check if the model is available (locally or can be downloaded)
    pub async fn is_available(&self) -> bool {
        match &self.source {
            ModelSource::Local(path) => path.exists(),
            ModelSource::HuggingFace { .. } => {
                // Try to download and see if it succeeds
                self.source.resolve().await.is_ok()
            }
        }
    }
}

/// Embedding provider that supports lazy initialization and fallback
pub enum LocalEmbeddingProvider {
    /// Full GGUF embedding model
    Gguf(LazyEmbeddingModel),
    /// FTS-only mode (no embeddings)
    FtsOnly { reason: String },
}

impl LocalEmbeddingProvider {
    /// Create a new provider with auto-detection and fallback
    pub async fn create(source: ModelSource, dimension: usize) -> Self {
        let model = LazyEmbeddingModel::new(source, dimension);

        // Check if model is available
        if model.is_available().await {
            LocalEmbeddingProvider::Gguf(model)
        } else {
            LocalEmbeddingProvider::FtsOnly {
                reason: "Model not available".to_string(),
            }
        }
    }

    /// Create with explicit FTS-only mode
    pub fn fts_only(reason: impl Into<String>) -> Self {
        LocalEmbeddingProvider::FtsOnly {
            reason: reason.into(),
        }
    }

    /// Check if this is FTS-only mode
    pub fn is_fts_only(&self) -> bool {
        matches!(self, LocalEmbeddingProvider::FtsOnly { .. })
    }

    /// Get the reason for FTS-only mode
    pub fn fts_reason(&self) -> Option<&str> {
        match self {
            LocalEmbeddingProvider::FtsOnly { reason } => Some(reason),
            _ => None,
        }
    }

    /// Generate embeddings (or return error if FTS-only)
    pub async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        match self {
            LocalEmbeddingProvider::Gguf(model) => model.embed_batch(texts).await,
            LocalEmbeddingProvider::FtsOnly { reason } => Err(
                crate::error::MantaError::Validation(format!(
                    "Embeddings unavailable (FTS-only mode): {}. To enable embeddings, configure a valid embedding model.",
                    reason
                ))
            ),
        }
    }

    /// Get model name (or "fts-only" for fallback mode)
    pub fn model_name(&self) -> &str {
        match self {
            LocalEmbeddingProvider::Gguf(model) => model.model_name(),
            LocalEmbeddingProvider::FtsOnly { .. } => "fts-only",
        }
    }

    /// Get dimension (or 0 for FTS-only)
    pub fn dimension(&self) -> usize {
        match self {
            LocalEmbeddingProvider::Gguf(model) => model.dimension(),
            LocalEmbeddingProvider::FtsOnly { .. } => 0,
        }
    }
}

// Import the EmbeddingProvider trait
use super::EmbeddingProvider;

#[async_trait::async_trait]
impl EmbeddingProvider for LocalEmbeddingProvider {
    fn model_name(&self) -> &str {
        self.model_name()
    }

    fn dimension(&self) -> usize {
        self.dimension()
    }

    async fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        self.embed_batch(texts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_source_parse_local() {
        let source = ModelSource::parse("/path/to/model.gguf");
        assert!(matches!(source, ModelSource::Local(_)));
    }

    #[test]
    fn test_model_source_parse_hf() {
        let source = ModelSource::parse("hf:org/model/file.gguf");
        assert!(matches!(source, ModelSource::HuggingFace { .. }));
    }

    #[test]
    fn test_model_source_parse_hf_shorthand() {
        let source = ModelSource::parse("org/model/file.gguf");
        assert!(matches!(source, ModelSource::HuggingFace { .. }));
    }
}
