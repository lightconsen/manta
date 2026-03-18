//! Echo Channel - Example WASM Plugin for Manta
//!
//! This is a simple example channel that echoes back any messages it receives.
//! It demonstrates the WASM plugin interface for Manta channels.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;

// Import the WIT-generated bindings
mod bindings {
    wit_bindgen::generate!({
        path: "../../../wit/channel.wit",
        world: "channel-plugin",
    });
}

use bindings::exports::manta::channel::channel::Guest;
use bindings::manta::channel::host::{self, LogLevel};

/// Plugin state
#[derive(Serialize, Deserialize)]
struct EchoConfig {
    prefix: String,
    include_timestamp: bool,
}

impl Default for EchoConfig {
    fn default() -> Self {
        Self {
            prefix: "Echo".to_string(),
            include_timestamp: true,
        }
    }
}

/// Channel state stored in thread-local
thread_local! {
    static STATE: RefCell<ChannelState> = RefCell::new(ChannelState::default());
}

#[derive(Default)]
struct ChannelState {
    config: EchoConfig,
    running: bool,
    message_count: u64,
}

/// The Echo Channel implementation
pub struct EchoChannel;

impl Guest for EchoChannel {
    /// Initialize the channel with configuration
    fn init(config: String) -> Result<String, String> {
        host::log(LogLevel::Info, "EchoChannel: Initializing...");

        let echo_config: EchoConfig = if config.is_empty() {
            EchoConfig::default()
        } else {
            serde_json::from_str(&config).map_err(|e| format!("Invalid config: {}", e))?
        };

        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.config = echo_config;
            state.running = false;
            state.message_count = 0;
        });

        host::log(LogLevel::Info, "EchoChannel: Initialized successfully");
        Ok("Echo channel ready".to_string())
    }

    /// Start the channel
    fn start() -> Result<(), String> {
        host::log(LogLevel::Info, "EchoChannel: Starting...");

        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.running = true;
        });

        // Simulate receiving a message (in real implementation, this would set up listeners)
        host::log(LogLevel::Info, "EchoChannel: Started and listening");
        Ok(())
    }

    /// Stop the channel
    fn stop() -> Result<(), String> {
        host::log(LogLevel::Info, "EchoChannel: Stopping...");

        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.running = false;
        });

        host::log(LogLevel::Info, "EchoChannel: Stopped");
        Ok(())
    }

    /// Get channel name
    fn get_name() -> String {
        "echo".to_string()
    }

    /// Get channel capabilities
    fn get_capabilities() -> bindings::exports::manta::channel::channel::Capabilities {
        bindings::exports::manta::channel::channel::Capabilities {
            supports_formatting: true,
            supports_attachments: false,
            supports_images: false,
            supports_threads: false,
            supports_typing: true,
            supports_buttons: false,
            supports_commands: true,
            supports_reactions: false,
        }
    }

    /// Send a message
    fn send(
        message: bindings::exports::manta::channel::channel::OutgoingMessage,
        _options: bindings::exports::manta::channel::channel::MessageOptions,
    ) -> Result<String, String> {
        let conversation_id = message.conversation_id;
        let content = message.content;

        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.message_count += 1;

            let prefix = &state.config.prefix;
            let timestamp = if state.config.include_timestamp {
                format!(" [{}]", std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs())
            } else {
                String::new()
            };

            // In a real channel, this would send to an external service
            // For echo, we just log and return
            let log_msg = format!(
                "{}:{} - {}: {}{}",
                prefix, state.message_count, conversation_id, content, timestamp
            );
            host::log(LogLevel::Info, &log_msg);

            // Echo the message back as an incoming message
            let incoming = bindings::manta::channel::host::IncomingMessage {
                id: format!("echo-{}", state.message_count),
                user_id: "echo-bot".to_string(),
                conversation_id: conversation_id.clone(),
                content: format!("Echo: {}", content),
                attachments: vec![],
            };
            host::receive_message(incoming);
        });

        Ok(format!("msg-{}", conversation_id))
    }

    /// Send typing indicator
    fn send_typing(_conversation_id: String) -> Result<(), String> {
        host::log(LogLevel::Debug, "EchoChannel: Typing indicator");
        Ok(())
    }

    /// Edit a message (not supported in echo)
    fn edit_message(_message_id: String, _new_content: String) -> Result<(), String> {
        host::log(LogLevel::Warn, "EchoChannel: Edit not supported");
        Ok(())
    }

    /// Delete a message (not supported in echo)
    fn delete_message(_message_id: String) -> Result<(), String> {
        host::log(LogLevel::Warn, "EchoChannel: Delete not supported");
        Ok(())
    }

    /// Health check
    fn health_check() -> Result<bool, String> {
        STATE.with(|state| {
            let state = state.borrow();
            Ok(state.running)
        })
    }
}

bindings::export!(EchoChannel with_types_in bindings);
