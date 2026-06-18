//! [`LocalLlamaSummarizer`] — small local GGUF model via llama-cpp-2.
//!
//! Feature-gated behind `local-summarizer`. Downloads
//! Qwen2.5-1.5B-Instruct-GGUF (Q4_K_M, ~1.0 GB) from HuggingFace Hub
//! on first use, caches in the default HF cache directory.
//!
//! # Per-call context
//!
//! A fresh [`LlamaContext`] is created for each `summarize()` call rather
//! than storing one in a `Mutex`, because `LlamaContext` borrows from
//! `LlamaModel` with a lifetime parameter that makes struct-level storage
//! awkward.  Context creation overhead is negligible at the 60 s tick rate.
//!
//! # Latency target
//!
//! < 250 ms per call on Apple Silicon (M-series).

use async_trait::async_trait;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::AddBos;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing::info;

use crate::perception::{AdapterError, PerceptionSummarizer};

// ── Model identity ──────────────────────────────────────────────────────────

/// HuggingFace repo for the default summarizer GGUF model.
const MODEL_REPO: &str = "Qwen/Qwen2.5-1.5B-Instruct-GGUF";
/// Filename within the repo (Q4_K_M for ~1.0 GB footprint).
const MODEL_FILENAME: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";
/// Max tokens to generate per summary call.
const MAX_GEN_TOKENS: u32 = 128;
/// Context size for the llama.cpp model.
const N_CTX: u32 = 2048;
/// Batch size for prompt processing and generation.
const N_BATCH: usize = 512;

// ── Global backend singleton ────────────────────────────────────────────────

static LLAMA_BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();

fn get_backend() -> Result<&'static LlamaBackend, AdapterError> {
    let result = LLAMA_BACKEND.get_or_init(|| {
        LlamaBackend::init().map_err(|e| format!("failed to init llama.cpp backend: {e}"))
    });
    match result {
        Ok(ref backend) => Ok(backend),
        Err(ref e) => Err(AdapterError::Summarizer(format!(
            "backend init failed: {e}"
        ))),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Download (or retrieve from cache) a HuggingFace model file.
async fn resolve_model(repo_id: &str, filename: &str) -> Result<PathBuf, AdapterError> {
    let api = hf_hub::api::tokio::Api::new().map_err(|e| {
        AdapterError::Summarizer(format!("failed to create HF API: {e}"))
    })?;
    let repo = api.model(repo_id.to_string());
    let local_path = repo.get(filename).await.map_err(|e| {
        AdapterError::Summarizer(format!("failed to download model {repo_id}/{filename}: {e}"))
    })?;
    info!("Model resolved to: {:?}", local_path);
    Ok(local_path)
}

/// Convert a sequence of generated tokens to a UTF-8 string.
fn tokens_to_string(model: &LlamaModel, tokens: &[llama_cpp_2::token::LlamaToken]) -> String {
    let mut raw = Vec::with_capacity(tokens.len() * 8);
    for &token in tokens {
        if let Ok(bytes) = model.token_to_piece_bytes(token, 16, false, None) {
            raw.extend_from_slice(&bytes);
        }
    }
    String::from_utf8_lossy(&raw).into_owned()
}

// ── LocalLlamaSummarizer ────────────────────────────────────────────────────

/// A [`PerceptionSummarizer`] backed by a local GGUF model via llama-cpp-2.
///
/// Construct via [`LocalLlamaSummarizer::new_auto`] (downloads the default
/// Qwen2.5-1.5B model from HuggingFace) or [`LocalLlamaSummarizer::from_path`]
/// (point at a local GGUF file).
pub struct LocalLlamaSummarizer {
    model: LlamaModel,
    backend: &'static LlamaBackend,
}

impl std::fmt::Debug for LocalLlamaSummarizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalLlamaSummarizer").finish_non_exhaustive()
    }
}

impl LocalLlamaSummarizer {
    /// Auto-download the default Qwen2.5-1.5B-Instruct Q4_K_M GGUF model
    /// from HuggingFace Hub and create a summarizer.
    pub async fn new_auto() -> Result<Self, AdapterError> {
        let path = resolve_model(MODEL_REPO, MODEL_FILENAME).await?;
        Self::from_path(path)
    }

    /// Create a summarizer from a local GGUF file path.
    pub fn from_path(path: PathBuf) -> Result<Self, AdapterError> {
        let backend = get_backend()?;
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(backend, &path, &model_params).map_err(|e| {
            AdapterError::Summarizer(format!("failed to load model from {path:?}: {e}"))
        })?;

        info!("LocalLlamaSummarizer loaded from {path:?}");
        Ok(Self { model, backend })
    }
}

#[async_trait]
impl PerceptionSummarizer for LocalLlamaSummarizer {
    async fn summarize(&self, system: &str, user: &str) -> Result<String, AdapterError> {
        // Build a ChatML prompt compatible with Qwen2.5 instruct models.
        let prompt = format!(
            "<|im_start|>system\n{}<|im_end|>\n\
             <|im_start|>user\n{}<|im_end|>\n\
             <|im_start|>assistant\n",
            system, user
        );

        let tokens = self
            .model
            .str_to_token(&prompt, AddBos::Never)
            .map_err(|e| AdapterError::Summarizer(format!("tokenization failed: {e}")))?;

        if tokens.is_empty() {
            return Err(AdapterError::Summarizer(
                "empty tokenization result".into(),
            ));
        }

        // Create a fresh context for this call (avoids LlamaContext lifetime
        // complexity — overhead is negligible at the 60 s tick rate).
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(N_CTX))
            .with_n_batch(N_BATCH as u32);
        let mut ctx = self.model.new_context(self.backend, ctx_params).map_err(|e| {
            AdapterError::Summarizer(format!("failed to create llama context: {e}"))
        })?;

        // ── Prompt evaluation ──────────────────────────────────────────────
        {
            let mut batch = LlamaBatch::new(N_BATCH, 1);
            for (i, &token) in tokens.iter().enumerate() {
                let is_last = i == tokens.len() - 1;
                batch.add(token, i as i32, &[0], is_last).map_err(|e| {
                    AdapterError::Summarizer(format!("failed to add prompt token: {e}"))
                })?;
            }
            ctx.decode(&mut batch).map_err(|e| {
                AdapterError::Summarizer(format!("prompt decode failed: {e}"))
            })?;
        }

        // ── Greedy generation loop ────────────────────────────────────────
        let mut sampler = LlamaSampler::greedy();
        let mut output_tokens: Vec<llama_cpp_2::token::LlamaToken> = Vec::new();
        let eos_token = self.model.token_eos();
        let n_prompt = tokens.len();

        for gen_idx in 0..MAX_GEN_TOKENS {
            // Sample from the last decoded position.
            let pos = n_prompt
                .checked_add(gen_idx as usize)
                .and_then(|p| p.checked_sub(1))
                .ok_or_else(|| {
                    AdapterError::Summarizer("position arithmetic overflow".into())
                })?;
            let next = sampler.sample(&ctx, pos as i32);

            if next == eos_token {
                break;
            }

            output_tokens.push(next);

            // Prepare next batch with the single generated token.
            let insert_pos = n_prompt
                .checked_add(gen_idx as usize)
                .ok_or_else(|| AdapterError::Summarizer("position overflow".into()))?;
            let mut batch = LlamaBatch::new(1, 1);
            batch
                .add(next, insert_pos as i32, &[0], true)
                .map_err(|e| AdapterError::Summarizer(format!("failed to add gen token: {e}")))?;
            ctx.decode(&mut batch).map_err(|e| {
                AdapterError::Summarizer(format!("gen decode failed: {e}"))
            })?;
        }

        let raw = tokens_to_string(&self.model, &output_tokens);
        Ok(raw.trim().to_string())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_path_errors_gracefully_on_invalid_file() {
        // Write a tiny invalid file so llama.cpp attempts loading
        // (file-not-found triggers a panic inside llama.cpp).
        let dir = std::env::temp_dir();
        let path = dir.join("__syscity_test_invalid_model.gguf");
        std::fs::write(&path, b"not a valid gguf model").ok();
        let result = LocalLlamaSummarizer::from_path(path.clone());
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err(), "expected Err for invalid file");
        match result {
            Err(AdapterError::Summarizer(msg)) => {
                assert!(
                    msg.contains("failed to load model"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected Summarizer error, got {other:?}"),
        }
    }

    #[tokio::test]
    #[ignore = "requires ~1 GB download and llama.cpp backend; run manually"]
    async fn e2e_auto_download_and_summarize() {
        let summarizer = LocalLlamaSummarizer::new_auto()
            .await
            .expect("model download + init should succeed");
        let result = summarizer
            .summarize("You are a helpful assistant.", "Say hello in one word.")
            .await
            .expect("summarize should succeed");
        assert!(!result.is_empty(), "summary should not be empty");
        // The model should output something like "Hello" or "Hi".
        eprintln!("Local LLM summary: {result}");
    }
}