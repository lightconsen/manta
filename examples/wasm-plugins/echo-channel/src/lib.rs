//! Echo Channel - Example WASM Plugin for Manta
//!
//! This is a simple example channel that echoes back any messages it receives.
//! It demonstrates the WASM plugin interface for Manta channels.

mod bindings {
    wit_bindgen::generate!({
        path: "../../../wit/channel.wit",
        world: "channel-plugin",
    });
}

use bindings::exports::manta::channel::channel::{
    Capabilities, Guest, MessageOptions, OutgoingMessage,
    StringResult, UnitResult, BoolResult,
};
use bindings::manta::channel::types::{
    ChatType, IncomingMessage, ChannelError,
};
use bindings::manta::channel::logging::LogLevel;
use bindings::manta::channel::host::{log, receive_message};

use serde::{Deserialize, Serialize};
use std::cell::RefCell;

/// Configuration for the echo channel
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct EchoConfig {
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_timestamp")]
    pub include_timestamp: bool,
}

fn default_prefix() -> String {
    "Echo".to_string()
}

fn default_timestamp() -> bool {
    true
}

/// Channel state
struct EchoChannel {
    config: EchoConfig,
    running: bool,
    message_count: u64,
}

impl Default for EchoChannel {
    fn default() -> Self {
        Self {
            config: EchoConfig::default(),
            running: false,
            message_count: 0,
        }
    }
}

thread_local! {
    static CHANNEL: RefCell<EchoChannel> = RefCell::new(EchoChannel::default());
}

/// Log helper
fn log_info(message: &str) {
    log(LogLevel::Info, message);
}

fn log_debug(message: &str) {
    log(LogLevel::Debug, message);
}

fn log_warn(message: &str) {
    log(LogLevel::Warn, message);
}

/// Implement the Guest trait for our channel
struct EchoGuest;

impl Guest for EchoGuest {
    fn init(config: String) -> StringResult {
        CHANNEL.with(|ch| {
            let mut channel = ch.borrow_mut();
            channel.config = if config.is_empty() {
                EchoConfig::default()
            } else {
                match serde_json::from_str(&config) {
                    Ok(cfg) => cfg,
                    Err(e) => return StringResult::Err(ChannelError::InvalidConfig(e.to_string())),
                }
            };
            channel.running = false;
            channel.message_count = 0;

            log_info(&format!("EchoChannel initialized with prefix '{}'", channel.config.prefix));
            StringResult::Ok("initialized".to_string())
        })
    }

    fn start() -> UnitResult {
        CHANNEL.with(|ch| {
            ch.borrow_mut().running = true;
        });
        log_info("EchoChannel started");
        UnitResult::Ok
    }

    fn stop() -> UnitResult {
        CHANNEL.with(|ch| {
            ch.borrow_mut().running = false;
        });
        log_info("EchoChannel stopped");
        UnitResult::Ok
    }

    fn get_name() -> String {
        "echo".to_string()
    }

    fn get_capabilities() -> Capabilities {
        Capabilities {
            chat_types: vec![ChatType::Direct, ChatType::Group],
            supports_formatting: true,
            supports_attachments: false,
            supports_images: false,
            supports_threads: false,
            supports_typing: true,
            supports_buttons: false,
            supports_commands: true,
            supports_reactions: false,
            supports_edit: false,
            supports_unsend: false,
            supports_effects: false,
        }
    }

    fn send(message: OutgoingMessage, _options: MessageOptions) -> StringResult {
        CHANNEL.with(|ch| {
            let mut channel = ch.borrow_mut();
            channel.message_count += 1;

            let timestamp = if channel.config.include_timestamp {
                format!(" [{}]", channel.message_count)
            } else {
                String::new()
            };

            // Log the message
            let log_msg = format!(
                "{}:{} - {}: {}{}",
                channel.config.prefix, channel.message_count,
                message.conversation_id, message.content, timestamp
            );
            log_info(&log_msg);

            // Echo the message back
            let echo_content = format!("{}: {}", channel.config.prefix, message.content);

            let incoming = IncomingMessage {
                id: format!("echo-{}", channel.message_count),
                user_id: "echo-bot".to_string(),
                conversation_id: message.conversation_id,
                content: echo_content,
                attachments: vec![],
            };

            receive_message(&incoming);

            StringResult::Ok(format!("msg-{}", channel.message_count))
        })
    }

    fn send_typing(_conversation_id: String) -> UnitResult {
        log_debug("Sending typing indicator");
        UnitResult::Ok
    }

    fn edit_message(_message_id: String, _new_content: String) -> UnitResult {
        log_warn("Edit not supported in echo channel");
        UnitResult::Err(ChannelError::NotSupported("Edit not supported".to_string()))
    }

    fn delete_message(_message_id: String) -> UnitResult {
        log_warn("Delete not supported in echo channel");
        UnitResult::Err(ChannelError::NotSupported("Delete not supported".to_string()))
    }

    fn health_check() -> BoolResult {
        let running = CHANNEL.with(|ch| ch.borrow().running);
        BoolResult::Ok(running)
    }
}

// Export the guest implementation
bindings::export!(EchoGuest with_types_in bindings);
