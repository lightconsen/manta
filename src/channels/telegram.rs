//! Telegram Channel Implementation
//!
//! This module implements the Channel trait for Telegram using teloxide.

use crate::channels::{
    Channel, ChannelCapabilities, ConversationId, FormattedContent, IncomingMessage,
    MessageMetadata, OutgoingMessage,
};
use crate::core::models::Id;
use crate::security::pairing::{DmPolicy, PairingStore, RequestAccessResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use tokio::sync::{mpsc, Notify, RwLock};
use tracing::{debug, error, info, warn};

#[cfg(feature = "telegram")]
use teloxide::{
    dispatching::{Dispatcher, UpdateFilterExt},
    payloads::SendMessageSetters,
    prelude::*,
    types::{Message, MessageId, ParseMode},
    Bot,
};

/// Telegram channel configuration
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Bot token from @BotFather
    pub token: String,
    /// Optional allowed usernames (empty = allow all)
    pub allowed_usernames: Vec<String>,
}

impl TelegramConfig {
    /// Create new config with token
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            allowed_usernames: Vec::new(),
        }
    }

    /// Set allowed usernames
    pub fn allow_usernames(mut self, usernames: Vec<String>) -> Self {
        self.allowed_usernames = usernames;
        self
    }
}

/// Telegram channel implementation
#[derive(Clone)]
pub struct TelegramChannel {
    config: TelegramConfig,
    bot: Arc<RwLock<Option<Bot>>>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Mapping from internal message ID to (chat_id, telegram_message_id)
    message_map: Arc<RwLock<HashMap<Id, (i64, i32)>>>,
    /// Shutdown notifier
    shutdown_notify: Arc<Notify>,
    /// Message sender for routing incoming messages to the gateway/agent
    message_tx: Arc<RwLock<Option<mpsc::UnboundedSender<IncomingMessage>>>>,
    /// Session mapping: chat_id -> session_uuid (for /new command support)
    session_map: Arc<RwLock<HashMap<i64, String>>>,
    /// Pairing store for DM access control
    pairing_store: Arc<RwLock<Option<Arc<PairingStore>>>>,
    /// DM policy for access control
    dm_policy: Arc<RwLock<DmPolicy>>,
    /// Allowlist for users (used with Allowlist policy)
    allow_from: Arc<RwLock<Vec<String>>>,
}

impl std::fmt::Debug for TelegramChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramChannel")
            .field("config", &self.config)
            .field("bot_initialized", &true) // Bot is always initialized after start()
            .field("running", &self.running)
            .field("message_map", &self.message_map)
            .field("shutdown_notify", &self.shutdown_notify)
            .field("has_message_queue", &"<async>")
            .field("dm_policy", &"<async>")
            .field("has_pairing_store", &"<async>")
            .finish()
    }
}

impl TelegramChannel {
    /// Create a new Telegram channel
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            config,
            bot: Arc::new(RwLock::new(None)),
            running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            message_map: Arc::new(RwLock::new(HashMap::new())),
            shutdown_notify: Arc::new(Notify::new()),
            message_tx: Arc::new(RwLock::new(None)),
            session_map: Arc::new(RwLock::new(HashMap::new())),
            pairing_store: Arc::new(RwLock::new(None)),
            dm_policy: Arc::new(RwLock::new(DmPolicy::Open)),
            allow_from: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Set the pairing store for DM access control
    pub async fn set_pairing_store(&self, store: Arc<PairingStore>) {
        let mut ps = self.pairing_store.write().await;
        *ps = Some(store);
    }

    /// Set the DM policy
    pub async fn set_dm_policy(&self, policy: DmPolicy) {
        let mut policy_guard = self.dm_policy.write().await;
        *policy_guard = policy;
    }

    /// Set the allowlist
    pub async fn set_allow_from(&self, allow_from: Vec<String>) {
        let mut af = self.allow_from.write().await;
        *af = allow_from;
    }

    /// Get or create a session UUID for a chat
    async fn get_or_create_session(&self, chat_id: i64) -> String {
        {
            let sessions = self.session_map.read().await;
            if let Some(session_id) = sessions.get(&chat_id) {
                return session_id.clone();
            }
        }
        // Create new session
        let new_session = uuid::Uuid::new_v4().to_string();
        let mut sessions = self.session_map.write().await;
        sessions.insert(chat_id, new_session.clone());
        new_session
    }

    /// Reset session for a chat (when /new is used)
    async fn reset_session(&self, chat_id: i64) -> String {
        let new_session = uuid::Uuid::new_v4().to_string();
        let mut sessions = self.session_map.write().await;
        sessions.insert(chat_id, new_session.clone());
        new_session
    }

    /// Set the message queue sender for routing incoming messages
    pub async fn set_message_sender(&self, sender: mpsc::UnboundedSender<IncomingMessage>) {
        let mut tx = self.message_tx.write().await;
        *tx = Some(sender);
    }

    /// Get the message sender if available
    async fn get_message_sender(&self) -> Option<mpsc::UnboundedSender<IncomingMessage>> {
        self.message_tx.read().await.clone()
    }
}

/// Pre-compiled regex patterns for markdown parsing
static RE_BOLD_DOUBLE_STAR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\*\*(.+?)\*\*").unwrap());
static RE_BOLD_DOUBLE_UNDERSCORE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"__(.+?)__").unwrap());
static RE_ITALIC_STAR: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\*([^*]+)\*").unwrap());
static RE_ITALIC_UNDERSCORE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"_([^_]+)_").unwrap());
static RE_CODE_INLINE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"`([^`]+)`").unwrap());
static RE_CODE_BLOCK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"```(\w+)?\n(.*?)```").unwrap());
static RE_STRIKETHROUGH: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"~~(.+?)~~").unwrap());
static RE_LINK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap());

impl TelegramChannel {
    /// Convert markdown to Telegram HTML
    fn markdown_to_telegram_html(text: &str) -> String {
        let mut result = text.to_string();

        // Escape HTML special characters first
        result = result
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");

        // Use placeholders to protect bold from being converted to italic
        let bold_placeholder = "\x00BOLD\x00";
        result = RE_BOLD_DOUBLE_STAR
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!(
                    "{}{}{}",
                    bold_placeholder,
                    caps.get(1).map(|m| m.as_str()).unwrap_or(""),
                    bold_placeholder
                )
            })
            .to_string();

        result = RE_BOLD_DOUBLE_UNDERSCORE
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!(
                    "{}{}{}",
                    bold_placeholder,
                    caps.get(1).map(|m| m.as_str()).unwrap_or(""),
                    bold_placeholder
                )
            })
            .to_string();

        // Process italic
        result = RE_ITALIC_STAR.replace_all(&result, "<i>$1</i>").to_string();
        result = RE_ITALIC_UNDERSCORE
            .replace_all(&result, "<i>$1</i>")
            .to_string();

        // Restore bold placeholders with actual HTML tags.
        // The placeholder wraps content as: \x00BOLD\x00text\x00BOLD\x00
        // Use a regex to capture the content between the two markers.
        let re_bold_restore =
            regex::Regex::new(r"\x00BOLD\x00(.+?)\x00BOLD\x00").expect("valid regex");
        result = re_bold_restore
            .replace_all(&result, "<b>$1</b>")
            .to_string();

        // Code: `text` -> <code>text</code>
        result = RE_CODE_INLINE
            .replace_all(&result, "<code>$1</code>")
            .to_string();

        // Code block: ```lang\ncode``` -> <pre><code class="language-lang">code</code></pre>
        result = RE_CODE_BLOCK
            .replace_all(&result, "<pre><code>$2</code></pre>")
            .to_string();

        // Strikethrough: ~~text~~ -> <s>text</s>
        result = RE_STRIKETHROUGH
            .replace_all(&result, "<s>$1</s>")
            .to_string();

        // Links: [text](url) -> <a href="url">text</a>
        result = RE_LINK
            .replace_all(&result, r#"<a href="$2">$1</a>"#)
            .to_string();

        result
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn name(&self) -> &str {
        "telegram"
    }

    fn capabilities(&self) -> ChannelCapabilities {
        ChannelCapabilities {
            chat_types: vec![
                crate::channels::ChatType::Direct,
                crate::channels::ChatType::Group,
                crate::channels::ChatType::Channel,
            ],
            supports_formatting: true,
            supports_attachments: true,
            supports_images: true,
            supports_threads: true,
            supports_typing: true,
            supports_buttons: true,
            supports_commands: true,
            supports_reactions: true,
            supports_edit: true,
            supports_unsend: true,
            supports_effects: false,
        }
    }

    async fn start(&self) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            info!("Starting Telegram channel");

            let bot = Bot::new(&self.config.token);

            // Store the bot in the shared lock
            {
                let mut bot_guard = self.bot.write().await;
                *bot_guard = Some(bot.clone());
            }

            let allowed_usernames = self.config.allowed_usernames.clone();
            let running = self.running.clone();
            let shutdown_notify = self.shutdown_notify.clone();
            let message_tx = self.get_message_sender().await;
            let session_map = self.session_map.clone();
            let pairing_store = self.pairing_store.clone();
            let dm_policy = self.dm_policy.clone();
            let allow_from = self.allow_from.clone();

            running.store(true, std::sync::atomic::Ordering::SeqCst);

            // Spawn the update dispatcher with captured message sender
            tokio::spawn(async move {
                let handler =
                    dptree::entry().branch(Update::filter_message().endpoint(
                        move |bot: Bot, msg: Message| {
                            let tx = message_tx.clone();
                            let allowed = allowed_usernames.clone();
                            let sessions = session_map.clone();
                            let ps = pairing_store.clone();
                            let policy = dm_policy.clone();
                            let af = allow_from.clone();
                            async move {
                                handle_message_with_sender(bot, msg, tx, allowed, sessions, ps, policy, af).await
                            }
                        },
                    ));

                let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
                    .enable_ctrlc_handler()
                    .build();

                dispatcher.dispatch().await;
            });

            info!("Telegram channel started, waiting for shutdown signal...");

            // Wait for shutdown signal
            shutdown_notify.notified().await;

            info!("Telegram channel shutting down");
            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            Err(crate::error::MantaError::Internal("Telegram feature not enabled".to_string()))
        }
    }

    async fn stop(&self) -> crate::Result<()> {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.shutdown_notify.notify_one();
        info!("Telegram channel stopped");
        Ok(())
    }

    async fn send(&self, message: OutgoingMessage) -> crate::Result<Id> {
        #[cfg(feature = "telegram")]
        {
            let bot = {
                let bot_guard = self.bot.read().await;
                bot_guard.as_ref().cloned().ok_or_else(|| {
                    crate::error::MantaError::Internal("Bot not initialized".to_string())
                })?
            };

            let chat_id_str = &message.conversation_id.0;
            info!("DEBUG: Telegram send - conversation_id='{}'", chat_id_str);

            let chat_id: i64 = chat_id_str.parse().map_err(|e| {
                error!("DEBUG: Failed to parse chat_id '{}': {:?}", chat_id_str, e);
                crate::error::MantaError::Validation(format!("Invalid chat ID: '{}'", chat_id_str))
            })?;

            // Format content
            let (text, parse_mode) = match message.formatted_content {
                Some(FormattedContent::Html(html)) => (html, Some(ParseMode::Html)),
                Some(FormattedContent::Markdown(md)) => {
                    (Self::markdown_to_telegram_html(&md), Some(ParseMode::Html))
                }
                _ => (message.content, None),
            };

            // Send message
            let mut req = bot.send_message(ChatId(chat_id), text);

            if let Some(mode) = parse_mode {
                req = req.parse_mode(mode);
            }

            if message.options.silent {
                req = req.disable_notification(true);
            }

            // Handle reply_to by looking up the Telegram message ID from our mapping
            if let Some(reply_id) = message.reply_to {
                let map = self.message_map.read().await;
                if let Some((_, telegram_msg_id)) = map.get(&reply_id) {
                    req = req.reply_to_message_id(teloxide::types::MessageId(*telegram_msg_id));
                } else {
                    debug!("Reply message ID {} not found in mapping", reply_id);
                }
            }

            let sent = req.await.map_err(|e| {
                crate::error::MantaError::Internal(format!("Telegram send error: {}", e))
            })?;

            // Store the message ID mapping for edit/delete operations
            let internal_id = Id::new();
            let telegram_msg_id = sent.id.0;
            {
                let mut map = self.message_map.write().await;
                map.insert(internal_id.clone(), (chat_id, telegram_msg_id));
            }

            Ok(internal_id)
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = message;
            Err(crate::error::MantaError::Internal("Telegram feature not enabled".to_string()))
        }
    }

    async fn send_typing(&self, conversation_id: &ConversationId) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            let bot = {
                let bot_guard = self.bot.read().await;
                bot_guard.as_ref().cloned().ok_or_else(|| {
                    crate::error::MantaError::Internal("Bot not initialized".to_string())
                })?
            };

            let chat_id: i64 = conversation_id
                .0
                .parse()
                .map_err(|_| crate::error::MantaError::Validation("Invalid chat ID".to_string()))?;

            bot.send_chat_action(ChatId(chat_id), teloxide::types::ChatAction::Typing)
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("Telegram typing error: {}", e))
                })?;

            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = conversation_id;
            Err(crate::error::MantaError::Internal("Telegram feature not enabled".to_string()))
        }
    }

    async fn edit_message(&self, message_id: Id, new_content: String) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            let bot = {
                let bot_guard = self.bot.read().await;
                bot_guard.as_ref().cloned().ok_or_else(|| {
                    crate::error::MantaError::Internal("Bot not initialized".to_string())
                })?
            };

            // Look up the chat_id and telegram message_id from our mapping
            let (chat_id, telegram_msg_id) = {
                let map = self.message_map.read().await;
                map.get(&message_id).copied().ok_or_else(|| {
                    crate::error::MantaError::Validation(format!(
                        "Message ID {} not found",
                        message_id
                    ))
                })?
            };

            // Edit the message
            bot.edit_message_text(ChatId(chat_id), MessageId(telegram_msg_id), new_content)
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("Telegram edit error: {}", e))
                })?;

            info!("Edited message {} in chat {}", telegram_msg_id, chat_id);
            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = (message_id, new_content);
            Err(crate::error::MantaError::Internal("Telegram feature not enabled".to_string()))
        }
    }

    async fn delete_message(&self, message_id: Id) -> crate::Result<()> {
        #[cfg(feature = "telegram")]
        {
            let bot = {
                let bot_guard = self.bot.read().await;
                bot_guard.as_ref().cloned().ok_or_else(|| {
                    crate::error::MantaError::Internal("Bot not initialized".to_string())
                })?
            };

            // Look up the chat_id and telegram message_id from our mapping
            let (chat_id, telegram_msg_id) = {
                let map = self.message_map.read().await;
                map.get(&message_id).copied().ok_or_else(|| {
                    crate::error::MantaError::Validation(format!(
                        "Message ID {} not found",
                        message_id
                    ))
                })?
            };

            // Delete the message
            bot.delete_message(ChatId(chat_id), MessageId(telegram_msg_id))
                .await
                .map_err(|e| {
                    crate::error::MantaError::Internal(format!("Telegram delete error: {}", e))
                })?;

            // Remove from mapping after successful deletion
            {
                let mut map = self.message_map.write().await;
                map.remove(&message_id);
            }

            info!("Deleted message {} from chat {}", telegram_msg_id, chat_id);
            Ok(())
        }

        #[cfg(not(feature = "telegram"))]
        {
            let _ = message_id;
            Err(crate::error::MantaError::Internal("Telegram feature not enabled".to_string()))
        }
    }

    async fn health_check(&self) -> crate::Result<bool> {
        #[cfg(feature = "telegram")]
        {
            let bot_guard = self.bot.read().await;
            if let Some(bot) = bot_guard.as_ref() {
                match bot.get_me().await {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                }
            } else {
                Ok(false)
            }
        }

        #[cfg(not(feature = "telegram"))]
        {
            Ok(false)
        }
    }
}

#[cfg(feature = "telegram")]
async fn handle_message_with_sender(
    bot: Bot,
    msg: Message,
    message_tx: Option<mpsc::UnboundedSender<IncomingMessage>>,
    allowed_usernames: Vec<String>,
    session_map: Arc<RwLock<HashMap<i64, String>>>,
    pairing_store: Arc<RwLock<Option<Arc<PairingStore>>>>,
    dm_policy: Arc<RwLock<DmPolicy>>,
    allow_from: Arc<RwLock<Vec<String>>>,
) -> ResponseResult<()> {
    if let Some(text) = msg.text() {
        let user = msg.from();
        let username: String = user
            .as_ref()
            .map(|u| u.username.clone().unwrap_or_else(|| u.first_name.clone()))
            .unwrap_or_else(|| "unknown".to_string());
        let chat_id = msg.chat.id.0;
        let user_id = msg.from().map(|u| u.id.0.to_string()).unwrap_or_default();

        // Check DM policy
        let policy = *dm_policy.read().await;

        match policy {
            DmPolicy::Open => {
                // Allow all - no checks needed
            }
            DmPolicy::Allowlist => {
                // Check if user is in allowlist
                let allow_list = allow_from.read().await;
                let is_allowed = allow_list.iter().any(|a| a == &user_id || a.eq_ignore_ascii_case(&username));

                if !is_allowed {
                    warn!("User @{} ({}) is not in allowlist", username, user_id);
                    bot.send_message(msg.chat.id, "🔒 This bot is private. You're not authorized to use it.")
                        .await?;
                    return Ok(());
                }
            }
            DmPolicy::Pairing => {
                // Check if user is authorized via pairing
                if let Some(store) = pairing_store.read().await.as_ref() {
                    if !store.is_authorized("telegram", &user_id).await {
                        // Not authorized - check if they already have a pending request
                        match store.request_access("telegram", &user_id, Some(&username)).await {
                            Ok(RequestAccessResult::AlreadyAuthorized) => {
                                // Shouldn't happen since we just checked, but allow through
                            }
                            Ok(RequestAccessResult::NewRequest { code }) => {
                                info!("New pairing request from @{} ({}): code={}", username, user_id, code);
                                bot.send_message(
                                    msg.chat.id,
                                    format!(
                                        "🔒 This bot requires pairing.\n\n\
                                        Your pairing code: **{}**\n\n\
                                        Please share this code with an admin to get access.\n\
                                        Or ask an admin to run:\n\
                                        `manta pairing approve telegram {}`",
                                        code, code
                                    ),
                                )
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                                return Ok(());
                            }
                            Ok(RequestAccessResult::AlreadyPending { code, created_at: _ }) => {
                                bot.send_message(
                                    msg.chat.id,
                                    format!(
                                        "⏳ Your pairing request is still pending.\n\n\
                                        Code: **{}**\n\n\
                                        Please wait for an admin to approve your request.",
                                        code
                                    ),
                                )
                                .parse_mode(ParseMode::MarkdownV2)
                                .await?;
                                return Ok(());
                            }
                            Ok(RequestAccessResult::RateLimited { .. }) => {
                                bot.send_message(
                                    msg.chat.id,
                                    "⏳ Too many pairing requests. Please try again later.",
                                )
                                .await?;
                                return Ok(());
                            }
                            Err(_) => {
                                bot.send_message(
                                    msg.chat.id,
                                    "❌ Error processing pairing request. Please try again later.",
                                )
                                .await?;
                                return Ok(());
                            }
                        }
                    }
                } else {
                    warn!("Pairing policy set but no pairing store configured");
                    bot.send_message(msg.chat.id, "🔒 Pairing is not configured. Please contact the admin.")
                        .await?;
                    return Ok(());
                }
            }
        }

        // Legacy: Also check allowed_usernames if not empty (for backward compatibility)
        if !allowed_usernames.is_empty() {
            let is_allowed = user
                .as_ref()
                .map(|u| u.username.as_ref())
                .flatten()
                .map(|u| allowed_usernames.iter().any(|a| a.eq_ignore_ascii_case(u)))
                .unwrap_or(false);

            if !is_allowed {
                warn!("User @{} is not in allowed usernames list", username);
                bot.send_message(msg.chat.id, "Sorry, you're not authorized to use this bot.")
                    .await?;
                return Ok(());
            }
        }

        // Handle /new command to start a fresh session
        if text.trim() == "/new" {
            let new_session = uuid::Uuid::new_v4().to_string();
            {
                let mut sessions = session_map.write().await;
                sessions.insert(chat_id, new_session.clone());
            }
            info!("🆕 New session started for @{}: {}", username, new_session);
            bot.send_message(
                msg.chat.id,
                format!(
                    "🆕 Started new session:\n`{}`\n\nYour conversation history is now fresh.",
                    new_session
                ),
            )
            .parse_mode(ParseMode::MarkdownV2)
            .await?;
            return Ok(());
        }

        info!("📨 Received message from @{}: {}", username, text);

        // Get or create session UUID for this chat
        let session_id = {
            let sessions = session_map.read().await;
            if let Some(sid) = sessions.get(&chat_id) {
                sid.clone()
            } else {
                drop(sessions);
                let new_session = uuid::Uuid::new_v4().to_string();
                let mut sessions = session_map.write().await;
                sessions.insert(chat_id, new_session.clone());
                new_session
            }
        };

        // Create incoming message with UUID session
        let user_id = msg.from().map(|u| u.id.0.to_string()).unwrap_or_default();

        let incoming = IncomingMessage::new(&user_id, &session_id, text).with_metadata(
            MessageMetadata::new()
                .with_extra("message_id", msg.id.0)
                .with_extra("chat_type", format!("{:?}", msg.chat.kind))
                .with_extra("telegram_chat_id", chat_id),
        );

        // Route to agent via message queue if available
        if let Some(tx) = message_tx {
            if let Err(e) = tx.send(incoming) {
                warn!("Failed to route message to agent: {}", e);
                bot.send_message(
                    msg.chat.id,
                    "Sorry, I couldn't process your message. Please try again.",
                )
                .await?;
            }
        } else {
            warn!("No message_tx configured for Telegram channel — message from @{} dropped", username);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_telegram_html() {
        let md = "**bold** and *italic* and `code`";
        let html = TelegramChannel::markdown_to_telegram_html(md);
        assert!(html.contains("<b>bold</b>"));
        assert!(html.contains("<i>italic</i>"));
        assert!(html.contains("<code>code</code>"));
    }

    #[test]
    fn test_telegram_config() {
        let config =
            TelegramConfig::new("test_token").allow_usernames(vec!["test_user".to_string()]);
        assert_eq!(config.token, "test_token");
        assert_eq!(config.allowed_usernames.len(), 1);
    }
}
