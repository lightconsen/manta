//! Echo Channel - Example WASM Plugin for Manta
//!
//! This is a simple example channel that echoes back any messages it receives.
//! It demonstrates the WASM plugin interface for Manta channels using the SDK.

use manta_channel_sdk::{
    export_channel, Capabilities, Channel, ChannelError, IncomingMessage, MessageOptions,
    OutgoingMessage, log_debug, log_info, log_warn, emit_message, Result,
};
use serde::{Deserialize, Serialize};

/// Configuration for the echo channel
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct EchoConfig {
    /// Prefix for echoed messages
    #[serde(default = "default_prefix")]
    pub prefix: String,
    /// Include timestamp in echoed messages
    #[serde(default = "default_timestamp")]
    pub include_timestamp: bool,
}

fn default_prefix() -> String {
    "Echo".to_string()
}

fn default_timestamp() -> bool {
    true
}

/// The Echo Channel implementation
#[derive(Default)]
pub struct EchoChannel {
    config: EchoConfig,
    running: bool,
    message_count: u64,
}

impl Channel for EchoChannel {
    type Config = EchoConfig;

    fn name(&self) -> &str {
        "echo"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
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

    fn init(&mut self, config: EchoConfig) -> Result<()> {
        self.config = config;
        self.running = false;
        self.message_count = 0;

        log_info(&format!(
            "EchoChannel initialized with prefix '{}'",
            self.config.prefix
        ));
        Ok(())
    }

    fn start(&mut self) -> Result<()> {
        self.running = true;
        log_info("EchoChannel started");
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.running = false;
        log_info("EchoChannel stopped");
        Ok(())
    }

    fn send(&mut self, message: OutgoingMessage, _options: MessageOptions) -> Result<String> {
        self.message_count += 1;

        let timestamp = if self.config.include_timestamp {
            format!(" [{}]", self.message_count)
        } else {
            String::new()
        };

        // Log the message
        let log_msg = format!(
            "{}:{} - {}: {}{}",
            self.config.prefix, self.message_count,
            message.conversation_id, message.content, timestamp
        );
        log_info(&log_msg);

        // Echo the message back
        let echo_content = format!("{}: {}", self.config.prefix, message.content);

        let incoming = IncomingMessage {
            id: format!("echo-{}", self.message_count),
            user_id: "echo-bot".to_string(),
            conversation_id: message.conversation_id,
            content: echo_content,
            attachments: vec![],
        };

        emit_message(incoming);

        Ok(format!("msg-{}", self.message_count))
    }

    fn send_typing(&mut self, _conversation_id: String) -> Result<()> {
        log_debug("Sending typing indicator");
        Ok(())
    }

    fn edit_message(&mut self, _message_id: String, _new_content: String) -> Result<()> {
        log_warn("Edit not supported in echo channel");
        Err(ChannelError::NotSupported("Edit not supported".to_string()))
    }

    fn delete_message(&mut self, _message_id: String) -> Result<()> {
        log_warn("Delete not supported in echo channel");
        Err(ChannelError::NotSupported("Delete not supported".to_string()))
    }

    fn health_check(&self) -> Result<bool> {
        Ok(self.running)
    }
}

// Export the channel
export_channel!(EchoChannel);
