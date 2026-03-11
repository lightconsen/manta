//! Error types for Manta
//!
//! This module defines all error types used throughout the application.
//! It uses `thiserror` for defining structured errors that can be
//! easily converted to user-facing messages.

use std::path::PathBuf;
use thiserror::Error;

/// The main error type for Manta operations
#[derive(Error, Debug)]
pub enum MantaError {
    /// Configuration-related errors
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    /// I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// HTTP client errors
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Serialization errors
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Validation errors
    #[error("Validation error: {0}")]
    Validation(String),

    /// Resource not found
    #[error("Resource not found: {resource}")]
    NotFound { resource: String },

    /// Storage errors (database, file system, etc.)
    #[error("Storage error: {context} - {details}")]
    Storage { context: String, details: String },

    /// Internal errors (should not be exposed to users)
    #[error("Internal error: {0}")]
    Internal(String),

    /// External service errors
    #[error("External service error: {source}")]
    ExternalService {
        source: String,
        #[source]
        cause: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// Configuration-specific errors
#[derive(Error, Debug)]
pub enum ConfigError {
    /// Failed to read config file
    #[error("Failed to read config file at '{path}': {source}")]
    FileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse config file
    #[error("Failed to parse config file: {0}")]
    Parse(String),

    /// Missing required configuration
    #[error("Missing required configuration: {0}")]
    Missing(String),

    /// Invalid configuration value
    #[error("Invalid configuration value for '{key}': {message}")]
    InvalidValue { key: String, message: String },

    /// Environment variable error
    #[error("Environment variable error: {0}")]
    Env(#[from] std::env::VarError),
}

/// Result type alias for Manta operations
pub type Result<T> = std::result::Result<T, MantaError>;

/// Extension trait for adding context to results
pub trait ResultExt<T, E> {
    /// Add context to an error
    fn with_context<F, C>(self, f: F) -> Result<T>
    where
        F: FnOnce() -> C,
        C: Into<String>;
}

impl<T> ResultExt<T, std::io::Error> for std::result::Result<T, std::io::Error> {
    fn with_context<F, C>(self, _f: F) -> Result<T>
    where
        F: FnOnce() -> C,
        C: Into<String>,
    {
        self.map_err(|e| MantaError::Io(e))
    }
}

impl From<toml::ser::Error> for MantaError {
    fn from(err: toml::ser::Error) -> Self {
        MantaError::Internal(format!("TOML serialization error: {}", err))
    }
}

impl From<serde_yaml::Error> for MantaError {
    fn from(err: serde_yaml::Error) -> Self {
        MantaError::Internal(format!("YAML error: {}", err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = MantaError::Validation("test error".to_string());
        assert_eq!(err.to_string(), "Validation error: test error");
    }

    #[test]
    fn test_config_error_display() {
        let err = ConfigError::Missing("api_key".to_string());
        assert_eq!(err.to_string(), "Missing required configuration: api_key");
    }
}
