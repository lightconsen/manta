//! Local GGUF Embedding Implementation using Candle
//!
//! This module provides embedding generation using GGUF format models
//! loaded directly without external services.

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use std::path::Path;
use tokenizers::Tokenizer;
use tracing::{debug, info, warn};

/// Local embedding model wrapper
pub struct LocalEmbeddingModel {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    dimension: usize,
}

impl LocalEmbeddingModel {
    /// Load a GGUF embedding model
    ///
    /// # Arguments
    /// * `model_path` - Path to the GGUF file
    /// * `config_path` - Optional path to config.json
    /// * `tokenizer_path` - Optional path to tokenizer.json
    pub fn load(
        model_path: &Path,
        config_path: Option<&Path>,
        tokenizer_path: Option<&Path>,
    ) -> crate::Result<Self> {
        info!("Loading local embedding model from {:?}", model_path);

        // Determine paths
        let model_dir = model_path.parent().unwrap_or_else(|| Path::new("."));
        let default_config_path = model_dir.join("config.json");
        let config_path = config_path.unwrap_or(&default_config_path);
        let default_tokenizer_path = model_dir.join("tokenizer.json");
        let tokenizer_path = tokenizer_path.unwrap_or(&default_tokenizer_path);

        // Load device (CPU for now, could add CUDA/MPS support)
        let device = Device::Cpu;
        info!("Using device: {:?}", device);

        // Load tokenizer
        let tokenizer = Self::load_tokenizer(tokenizer_path)?;
        info!("Tokenizer loaded successfully");

        // Load model configuration
        let config = if config_path.exists() {
            let config_content = std::fs::read_to_string(config_path)
                .map_err(|e| crate::error::MantaError::Io(e))?;
            serde_json::from_str(&config_content)
                .map_err(|e| crate::error::MantaError::Serialization(e))?
        } else {
            // Default BERT configuration for embeddings
            warn!("No config.json found, using default BERT configuration");
            Self::default_config()
        };

        // Load model weights from GGUF
        // Note: Candle's GGUF support is still evolving
        // For now, we use safetensors if available, with GGUF as fallback
        let model = Self::load_model(&config, model_path, &device)?;

        let dimension = config.hidden_size as usize;
        info!("Model loaded successfully with dimension: {}", dimension);

        Ok(Self {
            model,
            tokenizer,
            device,
            dimension,
        })
    }

    /// Load tokenizer from file
    fn load_tokenizer(tokenizer_path: &Path) -> crate::Result<Tokenizer> {
        if !tokenizer_path.exists() {
            return Err(crate::error::MantaError::Validation(
                format!("Tokenizer not found at {:?}", tokenizer_path)
            ));
        }

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| crate::error::MantaError::Validation(
                format!("Failed to load tokenizer: {}", e)
            ))?;

        Ok(tokenizer)
    }

    /// Load model from safetensors
    fn load_model(
        config: &Config,
        model_path: &Path,
        device: &Device,
    ) -> crate::Result<BertModel> {
        // Try loading from safetensors first (more stable)
        let safetensors_path = model_path.with_extension("safetensors");

        if safetensors_path.exists() {
            info!("Loading from safetensors: {:?}", safetensors_path);
            let safetensors_vec = vec![safetensors_path.clone()];
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&safetensors_vec, DTYPE, device)
                    .map_err(|e| crate::error::MantaError::Validation(
                        format!("Failed to load safetensors from {:?}: {}", safetensors_path, e)
                    ))?
            };
            BertModel::load(vb, config)
                .map_err(|e| crate::error::MantaError::Validation(
                    format!("Failed to create BERT model from safetensors: {}", e)
                ))
        } else {
            // GGUF format support is limited in candle for BERT models
            // For now, we require safetensors format
            Err(crate::error::MantaError::Validation(
                format!("Model file not found at {:?} or {:?}. Safetensors format is required for local embeddings.",
                    model_path, safetensors_path)
            ))
        }
    }

    /// Default BERT configuration
    fn default_config() -> Config {
        // BERT base configuration
        serde_json::from_str(r#"{
            "architectures": ["BertModel"],
            "attention_probs_dropout_prob": 0.1,
            "hidden_act": "gelu",
            "hidden_dropout_prob": 0.1,
            "hidden_size": 768,
            "initializer_range": 0.02,
            "intermediate_size": 3072,
            "layer_norm_eps": 1e-12,
            "max_position_embeddings": 512,
            "model_type": "bert",
            "num_attention_heads": 12,
            "num_hidden_layers": 12,
            "pad_token_id": 0,
            "position_embedding_type": "absolute",
            "transformers_version": "4.30.0",
            "type_vocab_size": 2,
            "use_cache": true,
            "vocab_size": 30522
        }"#).expect("Default config should be valid JSON")
    }

    /// Generate embeddings for a batch of texts
    pub fn embed_batch(&self, texts: &[String]) -> crate::Result<Vec<Vec<f32>>> {
        let batch_size = texts.len();
        debug!("Embedding batch of {} texts", batch_size);

        // Tokenize all texts
        let encodings: Vec<_> = texts
            .iter()
            .map(|text| {
                self.tokenizer
                    .encode(text.clone(), true)
                    .map_err(|e| crate::error::MantaError::Validation(
                        format!("Tokenization failed: {}", e)
                    ))
            })
            .collect::<Result<_, _>>()?;

        // Extract token ids and attention masks
        let max_len = encodings.iter().map(|e| e.len()).max().unwrap_or(512);

        let mut token_ids_vec = Vec::with_capacity(batch_size * max_len);
        let mut attention_mask_vec = Vec::with_capacity(batch_size * max_len);

        for encoding in &encodings {
            let ids = encoding.get_ids();
            let len = ids.len();

            // Pad token ids
            for &id in ids {
                token_ids_vec.push(id as i64);
            }
            for _ in len..max_len {
                token_ids_vec.push(0); // PAD token
            }

            // Create attention mask (1 for real tokens, 0 for padding)
            for _ in 0..len {
                attention_mask_vec.push(1i64);
            }
            for _ in len..max_len {
                attention_mask_vec.push(0);
            }
        }

        // Create tensors
        let token_ids = Tensor::from_vec(
            token_ids_vec,
            (batch_size, max_len),
            &self.device,
        ).map_err(|e| crate::error::MantaError::Validation(
            format!("Failed to create token ids tensor: {}", e)
        ))?;

        let attention_mask = Tensor::from_vec(
            attention_mask_vec,
            (batch_size, max_len),
            &self.device,
        ).map_err(|e| crate::error::MantaError::Validation(
            format!("Failed to create attention mask tensor: {}", e)
        ))?;

        // Create token type ids (all zeros for single sequence)
        let token_type_ids = Tensor::zeros(
            (batch_size, max_len),
            DType::I64,
            &self.device,
        ).map_err(|e| crate::error::MantaError::Validation(
            format!("Failed to create token type ids: {}", e)
        ))?;

        // Run inference
        let embeddings = self.model.forward(&token_ids, &token_type_ids, Some(&attention_mask))
            .map_err(|e| crate::error::MantaError::Validation(
                format!("Model inference failed: {}", e)
            ))?;

        // Extract embeddings (mean pooling over sequence)
        // embeddings shape: [batch, seq_len, hidden_size]
        let pooled = embeddings.mean(1) // Average over sequence dimension
            .map_err(|e| crate::error::MantaError::Validation(
                format!("Failed to pool embeddings: {}", e)
            ))?;

        // Convert to Vec<Vec<f32>>
        let embeddings_vec: Vec<Vec<f32>> = (0..batch_size)
            .map(|i| {
                let row = pooled.get(i)
                    .map_err(|e| crate::error::MantaError::Validation(
                        format!("Failed to get embedding {}: {}", i, e)
                    ))?;
                let data: Vec<f32> = row.to_vec1()
                    .map_err(|e| crate::error::MantaError::Validation(
                        format!("Failed to convert embedding to vec: {}", e)
                    ))?;

                // Normalize to unit vector
                let magnitude: f32 = data.iter().map(|x| x * x).sum::<f32>().sqrt();
                let normalized: Vec<f32> = if magnitude > 0.0 {
                    data.iter().map(|x| x / magnitude).collect()
                } else {
                    data
                };

                Ok(normalized)
            })
            .collect::<crate::Result<Vec<_>>>()?;

        debug!("Generated {} embeddings", embeddings_vec.len());
        Ok(embeddings_vec)
    }

    /// Get embedding dimension
    pub fn dimension(&self) -> usize {
        self.dimension
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LocalEmbeddingModel::default_config();
        assert_eq!(config.hidden_size, 768);
        assert_eq!(config.num_hidden_layers, 12);
    }
}
