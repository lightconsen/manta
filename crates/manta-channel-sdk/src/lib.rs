//! Manta Channel SDK
//!
//! This SDK provides types for building channel plugins
//! for Manta using Rust and WebAssembly.
//!
//! # Usage
//!
//! Add this to your `Cargo.toml`:
//! ```toml
//! [dependencies]
//! manta-channel-sdk = { path = "path/to/sdk" }
//! wit-bindgen = "0.54"
//! serde = { version = "1.0", features = ["derive"] }
//! serde_json = "1.0"
//!
//! [lib]
//! crate-type = ["cdylib"]
//! ```
//!
//! Then in your `src/lib.rs`:
//! ```rust
//! use manta_channel_sdk::{
//!     Capabilities, Guest, MessageOptions, OutgoingMessage,
//!     StringResult, UnitResult, BoolResult,
//!     ChatType, IncomingMessage, ChannelError,
//!     LogLevel, log, receive_message,
//! };
//!
//! // Generate bindings from WIT
//! mod bindings {
//!     wit_bindgen::generate!({
//!         path: "path/to/manta/wit/channel.wit",
//!         world: "channel-plugin",
//!     });
//! }
//!
//! use bindings::exports::manta::channel::channel::Guest;
//!
//! struct MyChannel;
//!
//! impl Guest for MyChannel {
//!     fn init(config: String) -> bindings::exports::manta::channel::channel::StringResult {
//!         // Initialize
//!         bindings::exports::manta::channel::channel::StringResult::Ok("ok".to_string())
//!     }
//!     // ... implement other methods
//! }
//!
//! bindings::export!(MyChannel);
//! ```

// Re-export types from generated bindings
// Note: Users need to generate their own bindings using wit_bindgen::generate!
// This crate provides the type definitions for convenience.

/// Path to the WIT file for generating bindings
pub const WIT_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../wit/channel.wit");

/// Re-exported types for convenience
///
/// These are the types you'll use when implementing your channel plugin.
/// Generate bindings in your crate with:
/// ```rust
/// wit_bindgen::generate!({
///     path: "path/to/manta/wit/channel.wit",
///     world: "channel-plugin",
/// });
/// ```
pub mod types {
    /// Placeholder types - these will be replaced by wit_bindgen in your crate
    ///
    /// When you generate bindings, use the types from your generated `bindings` module.

    /// Chat type enum
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ChatType {
        Direct,
        Group,
        Channel,
        Thread,
    }

    /// Capabilities struct
    #[derive(Debug, Clone)]
    pub struct Capabilities {
        pub chat_types: Vec<ChatType>,
        pub supports_formatting: bool,
        pub supports_attachments: bool,
        pub supports_images: bool,
        pub supports_threads: bool,
        pub supports_typing: bool,
        pub supports_buttons: bool,
        pub supports_commands: bool,
        pub supports_reactions: bool,
        pub supports_edit: bool,
        pub supports_unsend: bool,
        pub supports_effects: bool,
    }
}

/// Default capabilities for a basic channel
pub fn default_capabilities() -> types::Capabilities {
    types::Capabilities {
        chat_types: vec![types::ChatType::Direct],
        supports_formatting: true,
        supports_attachments: false,
        supports_images: false,
        supports_threads: false,
        supports_typing: true,
        supports_buttons: false,
        supports_commands: false,
        supports_reactions: false,
        supports_edit: false,
        supports_unsend: false,
        supports_effects: false,
    }
}
