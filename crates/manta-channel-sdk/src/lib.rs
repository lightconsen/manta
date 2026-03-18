//! Manta Channel SDK
//!
//! This SDK provides a convenient interface for building channel plugins
//! for Manta using Rust and WebAssembly.

pub mod bindings {
    wit_bindgen::generate!({
        path: "../../../wit/channel.wit",
        world: "channel-plugin",
    });
}

pub use bindings::exports::manta::channel::channel::{
    Capabilities, Guest as ChannelGuest, MessageOptions, OutgoingMessage,
};
pub use bindings::manta::channel::host::{
    get_config, log, receive_message, LogLevel,
};
pub use bindings::manta::channel::channel::{
    Attachment, IncomingMessage,
};

use std::cell::RefCell;

/// Result type for channel operations
pub type Result<T> = std::result::Result<T, ChannelError>;

/// Channel error types
#[derive(Debug, Clone)]
pub enum ChannelError {
    Generic(String),
    NotConfigured,
    SendFailed(String),
    InvalidConfig(String),
    NotSupported(String),
    RateLimited(u64),
}

impl std::fmt::Display for ChannelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelError::Generic(msg) => write!(f, "{}", msg),
            ChannelError::NotConfigured => write!(f, "Channel not configured"),
            ChannelError::SendFailed(msg) => write!(f, "Send failed: {}", msg),
            ChannelError::InvalidConfig(msg) => write!(f, "Invalid config: {}", msg),
            ChannelError::NotSupported(msg) => write!(f, "Not supported: {}", msg),
            ChannelError::RateLimited(secs) => write!(f, "Rate limited, retry in {}s", secs),
        }
    }
}

impl std::error::Error for ChannelError {}

impl From<ChannelError> for bindings::manta::channel::channel::ChannelError {
    fn from(err: ChannelError) -> Self {
        match err {
            ChannelError::Generic(msg) => Self::Generic(msg),
            ChannelError::NotConfigured => Self::NotConfigured,
            ChannelError::SendFailed(msg) => Self::SendFailed(msg),
            ChannelError::InvalidConfig(msg) => Self::InvalidConfig(msg),
            ChannelError::NotSupported(msg) => Self::NotSupported(msg),
            ChannelError::RateLimited(secs) => Self::RateLimited(secs),
        }
    }
}

/// Channel state wrapper
pub struct ChannelState<T: Channel> {
    pub inner: T,
    pub config: T::Config,
}

thread_local! {
    static STATE: RefCell<Option<Box<dyn std::any::Any>>> = RefCell::new(None);
}

/// Trait that channel implementations must satisfy
pub trait Channel: 'static {
    type Config: serde::de::DeserializeOwned + Default;

    fn name(&self) -> &str;
    fn capabilities(&self) -> Capabilities;
    fn init(&mut self, config: Self::Config) -> Result<()>;
    fn start(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn send(&mut self, message: OutgoingMessage, options: MessageOptions) -> Result<String>;
    fn send_typing(&mut self, conversation_id: String) -> Result<()>;
    fn edit_message(&mut self, message_id: String, new_content: String) -> Result<()>;
    fn delete_message(&mut self, message_id: String) -> Result<()>;
    fn health_check(&self) -> Result<bool>;
}

/// Macro to export a channel implementation
#[macro_export]
macro_rules! export_channel {
    ($channel:ty) => {
        use $crate::bindings::exports::manta::channel::channel::Guest;

        struct GuestImpl;

        impl Guest for GuestImpl {
            fn init(config: String) -> $crate::bindings::manta::channel::channel::Result<String, $crate::bindings::manta::channel::channel::ChannelError> {
                let parsed_config: <$channel as $crate::Channel>::Config = if config.is_empty() {
                    Default::default()
                } else {
                    serde_json::from_str(&config).map_err(|e| {
                        $crate::bindings::manta::channel::channel::ChannelError::InvalidConfig(e.to_string())
                    })?
                };

                let mut channel = <$channel>::default();
                $crate::Channel::init(&mut channel, parsed_config)
                    .map_err(|e| e.into())?;

                // Store the channel in thread-local storage
                $crate::store_channel(Box::new(channel));

                Ok(String::from("initialized"))
            }

            fn start() -> $crate::bindings::manta::channel::channel::Result<(), $crate::bindings::manta::channel::channel::ChannelError> {
                $crate::with_channel(|channel: &mut $channel| {
                    $crate::Channel::start(channel)
                }).map_err(|e| e.into())
            }

            fn stop() -> $crate::bindings::manta::channel::channel::Result<(), $crate::bindings::manta::channel::channel::ChannelError> {
                $crate::with_channel(|channel: &mut $channel| {
                    $crate::Channel::stop(channel)
                }).map_err(|e| e.into())
            }

            fn get_name() -> String {
                $crate::with_channel(|channel: &$channel| {
                    String::from($crate::Channel::name(channel))
                }).unwrap_or_else(|_| String::from("unknown"))
            }

            fn get_capabilities() -> $crate::bindings::exports::manta::channel::channel::Capabilities {
                $crate::with_channel(|channel: &$channel| {
                    $crate::Channel::capabilities(channel)
                }).unwrap_or_else(|_| $crate::bindings::exports::manta::channel::channel::Capabilities {
                    supports_formatting: false,
                    supports_attachments: false,
                    supports_images: false,
                    supports_threads: false,
                    supports_typing: false,
                    supports_buttons: false,
                    supports_commands: false,
                    supports_reactions: false,
                })
            }

            fn send(
                message: $crate::bindings::exports::manta::channel::channel::OutgoingMessage,
                options: $crate::bindings::exports::manta::channel::channel::MessageOptions,
            ) -> $crate::bindings::manta::channel::channel::Result<String, $crate::bindings::manta::channel::channel::ChannelError> {
                $crate::with_channel(|channel: &mut $channel| {
                    $crate::Channel::send(channel, message, options)
                }).map_err(|e| e.into())
            }

            fn send_typing(conversation_id: String) -> $crate::bindings::manta::channel::channel::Result<(), $crate::bindings::manta::channel::channel::ChannelError> {
                $crate::with_channel(|channel: &mut $channel| {
                    $crate::Channel::send_typing(channel, conversation_id)
                }).map_err(|e| e.into())
            }

            fn edit_message(message_id: String, new_content: String) -> $crate::bindings::manta::channel::channel::Result<(), $crate::bindings::manta::channel::channel::ChannelError> {
                $crate::with_channel(|channel: &mut $channel| {
                    $crate::Channel::edit_message(channel, message_id, new_content)
                }).map_err(|e| e.into())
            }

            fn delete_message(message_id: String) -> $crate::bindings::manta::channel::channel::Result<(), $crate::bindings::manta::channel::channel::ChannelError> {
                $crate::with_channel(|channel: &mut $channel| {
                    $crate::Channel::delete_message(channel, message_id)
                }).map_err(|e| e.into())
            }

            fn health_check() -> $crate::bindings::manta::channel::channel::Result<bool, $crate::bindings::manta::channel::channel::ChannelError> {
                $crate::with_channel(|channel: &$channel| {
                    $crate::Channel::health_check(channel)
                }).map_err(|e| e.into())
            }
        }

        $crate::bindings::export!(GuestImpl with_types_in $crate::bindings);
    };
}

/// Store a channel in thread-local storage
pub fn store_channel<T: Channel>(channel: T) {
    STATE.with(|state| {
        *state.borrow_mut() = Some(Box::new(channel));
    });
}

/// Execute a function with a reference to the stored channel
pub fn with_channel<T, F, R>(f: F) -> Result<R>
where
    F: FnOnce(&T) -> Result<R>,
    T: Channel,
{
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(channel) = state.as_mut() {
            if let Some(typed) = channel.downcast_ref::<T>() {
                // SAFETY: We know the type is correct
                let typed_ref = unsafe { &*(typed as *const T) };
                f(typed_ref)
            } else {
                Err(ChannelError::Generic("Invalid channel type".to_string()))
            }
        } else {
            Err(ChannelError::NotConfigured)
        }
    })
}

/// Execute a function with a mutable reference to the stored channel
pub fn with_channel_mut<T, F, R>(f: F) -> Result<R>
where
    F: FnOnce(&mut T) -> Result<R>,
    T: Channel,
{
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(channel) = state.as_mut() {
            if let Some(typed) = channel.downcast_mut::<T>() {
                f(typed)
            } else {
                Err(ChannelError::Generic("Invalid channel type".to_string()))
            }
        } else {
            Err(ChannelError::NotConfigured)
        }
    })
}

/// Helper function to receive an incoming message
pub fn emit_message(message: IncomingMessage) {
    receive_message(&message);
}

/// Helper function to log at debug level
pub fn log_debug(message: &str) {
    log(LogLevel::Debug, message);
}

/// Helper function to log at info level
pub fn log_info(message: &str) {
    log(LogLevel::Info, message);
}

/// Helper function to log at warn level
pub fn log_warn(message: &str) {
    log(LogLevel::Warn, message);
}

/// Helper function to log at error level
pub fn log_error(message: &str) {
    log(LogLevel::Error, message);
}

/// Get the host-provided configuration as a typed struct
pub fn get_typed_config<T: serde::de::DeserializeOwned>() -> Result<T> {
    let config_str = get_config();
    serde_json::from_str(&config_str)
        .map_err(|e| ChannelError::InvalidConfig(e.to_string()))
}
