//! Manta - Personal AI Assistant
//!
//! Manta is a lightweight, fast, and secure Personal AI Assistant written in Rust.
//! It combines the simplicity philosophy of NanoClaw with the performance
//! characteristics of ZeroClaw.
//!
//! # Architecture
//!
//! - **Core** (`core`): Domain models and business logic
//! - **Providers** (`providers`): LLM provider abstractions (OpenAI, Anthropic, etc.)
//! - **Channels** (`channels`): Communication interfaces (CLI, Telegram, Discord, etc.)
//! - **Tools** (`tools`): Capabilities for the AI to interact with the world
//! - **Adapters** (`adapters`): External service integrations
//! - **Config** (`config`): Configuration management
//! - **CLI** (`cli`): Command-line interface
//! - **Utils** (`utils`): Shared utilities
//!
//! # Example Usage
//!
//! ```rust
//! use manta::config::Config;
//! use manta::providers::{Message, Role, CompletionRequest};
//!
//! # async fn example() -> manta::error::Result<()> {
//! let config = Config::load()?;
//! // ... use providers, channels, tools
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![warn(rust_2018_idioms)]
// Unsafe code is allowed only for platform-specific resource limits
// All unsafe blocks are documented and justified

pub mod adapters;
pub mod agent;
pub mod assistants;
pub mod channels;
pub mod cli;
pub mod client;
pub mod config;
pub mod core;
pub mod cron;
pub mod daemon;
pub mod error;
pub mod logs;
pub mod memory;
pub mod providers;
pub mod security;
pub mod server;
pub mod skills;
pub mod tools;
pub mod web;
pub mod utils;

// Re-export commonly used types
pub use crate::core::Engine;
pub use config::{Config, ConfigWatcher, ReloadableConfig};
pub use error::{MantaError, Result};

/// Application version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Application name
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Application description
pub const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");

/// Application authors
pub const AUTHORS: &str = env!("CARGO_PKG_AUTHORS");

/// Check if the application is running in a production environment
pub fn is_production() -> bool {
    std::env::var("MANTA_ENV")
        .map(|v| v == "production")
        .unwrap_or(false)
}

/// Get the current environment name
pub fn environment() -> String {
    std::env::var("MANTA_ENV").unwrap_or_else(|_| "development".to_string())
}

/// Initialize the application
///
/// This function sets up logging, panic handlers, and other
/// global initialization.
pub fn init() -> Result<()> {
    utils::logging::setup_panic_handler();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
        assert!(!NAME.is_empty());
    }

    #[test]
    fn test_environment() {
        // Should return development by default
        let env = environment();
        assert!(
            env == "development" || !std::env::var("MANTA_ENV").unwrap_or_default().is_empty()
        );
    }
}
