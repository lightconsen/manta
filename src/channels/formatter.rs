//! Message Formatting for Different Channels
//!
//! This module provides formatters to convert markdown to channel-specific formats.

/// A formatter that converts markdown to channel-specific formats
pub trait MessageFormatter: Send + Sync {
    /// Format markdown text for this channel
    fn format(&self, text: &str) -> String;

    /// Format a code block
    fn format_code_block(&self, code: &str, language: Option<&str>) -> String;

    /// Format inline code
    fn format_inline_code(&self, code: &str) -> String;

    /// Format bold text
    fn format_bold(&self, text: &str) -> String;

    /// Format italic text
    fn format_italic(&self, text: &str) -> String;

    /// Format a link
    fn format_link(&self, text: &str, url: &str) -> String;

    /// Format a mention
    fn format_mention(&self, user_id: &str) -> String;

    /// Escape special characters
    fn escape(&self, text: &str) -> String;
}

/// Telegram HTML formatter
pub struct TelegramHtmlFormatter;

impl TelegramHtmlFormatter {
    /// Create a new formatter
    pub fn new() -> Self {
        Self
    }

    /// Escape HTML special characters
    fn escape_html(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }
}

impl Default for TelegramHtmlFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageFormatter for TelegramHtmlFormatter {
    fn format(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Code blocks first (to protect content inside)
        result = regex::Regex::new(r"```(\w+)?\n(.*?)```")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                let lang = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let code = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                self.format_code_block(code, Some(lang))
            })
            .to_string();

        // Bold
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                self.format_bold(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();

        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                self.format_bold(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();

        // Italic: *text* -> <i>text</i>
        // Use placeholders to protect bold from being converted
        let bold_placeholder = "\x00BOLD\x00";
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        // Now process italic
        result = regex::Regex::new(r"\*([^*]+)\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                self.format_italic(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();

        // Restore bold and apply formatting
        result = result.replace(bold_placeholder, "**");

        // Inline code
        result = regex::Regex::new(r"`([^`]+)`")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                self.format_inline_code(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();

        // Strikethrough
        result = regex::Regex::new(r"~~(.+?)~~")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("<s>{}</s>", Self::escape_html(caps.get(1).map(|m| m.as_str()).unwrap_or("")))
            })
            .to_string();

        // Links
        result = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                let text = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let url = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                self.format_link(text, url)
            })
            .to_string();

        result
    }

    fn format_code_block(&self, code: &str, _language: Option<&str>) -> String {
        format!("<pre><code>{}</code></pre>", Self::escape_html(code))
    }

    fn format_inline_code(&self, code: &str) -> String {
        format!("<code>{}</code>", Self::escape_html(code))
    }

    fn format_bold(&self, text: &str) -> String {
        format!("<b>{}</b>", Self::escape_html(text))
    }

    fn format_italic(&self, text: &str) -> String {
        format!("<i>{}</i>", Self::escape_html(text))
    }

    fn format_link(&self, text: &str, url: &str) -> String {
        format!(r#"<a href="{}">{}</a>"#, url, Self::escape_html(text))
    }

    fn format_mention(&self, user_id: &str) -> String {
        // Telegram mentions use the user ID
        format!(r#"<a href="tg://user?id={}">@user</a>"#, user_id)
    }

    fn escape(&self, text: &str) -> String {
        Self::escape_html(text)
    }
}

/// Discord markdown formatter
pub struct DiscordFormatter;

impl DiscordFormatter {
    /// Create a new formatter
    pub fn new() -> Self {
        Self
    }

    /// Escape Discord markdown characters
    fn escape_discord(text: &str) -> String {
        text.replace('\\', "\\\\")
            .replace('*', "\\*")
            .replace('_', "\\_")
            .replace('~', "\\~")
            .replace('`', "\\`")
            .replace('|', "\\|")
            .replace('[', "\\[")
            .replace(']', "\\]")
    }
}

impl Default for DiscordFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageFormatter for DiscordFormatter {
    fn format(&self, text: &str) -> String {
        // Discord supports standard markdown well
        // We just need to handle some edge cases
        let result = text.to_string();

        // Strikethrough: ~~text~~ (Discord supports this natively)
        // Underline: __text__ (Discord supports this natively)
        // Spoiler: ||text|| (Discord specific)

        // Mentions - convert @username to <@user_id> if possible
        // For now, leave as-is

        result
    }

    fn format_code_block(&self, code: &str, language: Option<&str>) -> String {
        let lang = language.unwrap_or("");
        format!("```{lang}\n{code}\n```")
    }

    fn format_inline_code(&self, code: &str) -> String {
        format!("`{code}`")
    }

    fn format_bold(&self, text: &str) -> String {
        format!("**{text}**")
    }

    fn format_italic(&self, text: &str) -> String {
        format!("*{text}*")
    }

    fn format_link(&self, text: &str, url: &str) -> String {
        format!("[{text}]({url})")
    }

    fn format_mention(&self, user_id: &str) -> String {
        format!("<@{user_id}>")
    }

    fn escape(&self, text: &str) -> String {
        Self::escape_discord(text)
    }
}

/// Slack mrkdwn formatter
pub struct SlackFormatter;

impl SlackFormatter {
    /// Create a new formatter
    pub fn new() -> Self {
        Self
    }

    /// Escape Slack mrkdwn characters
    fn escape_slack(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

impl Default for SlackFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageFormatter for SlackFormatter {
    fn format(&self, text: &str) -> String {
        let mut result = text.to_string();

        // Code blocks (protect first)
        result = regex::Regex::new(r"```(\w+)?\n(.*?)```")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                let lang = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let code = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                self.format_code_block(code, Some(lang))
            })
            .to_string();

        // Bold: **text** -> <b>text</b> temporarily to protect from italic conversion
        // Use placeholders to avoid conflicts with italic processing
        let bold_placeholder = "\x00BOLD\x00";
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        // Also handle __bold__
        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        // Italic: *text* -> _text_
        result = regex::Regex::new(r"\*([^*]+)\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                self.format_italic(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();

        // Italic: _text_ -> _text_ (keep as is)
        result = regex::Regex::new(r"_([^_]+)_")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                self.format_italic(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();

        // Restore bold placeholders to *text*
        result = result.replace(bold_placeholder, "*");

        // Inline code
        result = regex::Regex::new(r"`([^`]+)`")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                self.format_inline_code(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
            })
            .to_string();

        // Strikethrough: ~~text~~ -> ~text~
        result = regex::Regex::new(r"~~(.+?)~~")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("~{}~", Self::escape_slack(caps.get(1).map(|m| m.as_str()).unwrap_or("")))
            })
            .to_string();

        // Links: [text](url) -> <url|text>
        result = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                let text = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let url = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                self.format_link(text, url)
            })
            .to_string();

        result
    }

    fn format_code_block(&self, code: &str, language: Option<&str>) -> String {
        let lang = language.unwrap_or("");
        format!("```{lang}\n{code}\n```")
    }

    fn format_inline_code(&self, code: &str) -> String {
        format!("`{code}`")
    }

    fn format_bold(&self, text: &str) -> String {
        format!("*{}*", Self::escape_slack(text))
    }

    fn format_italic(&self, text: &str) -> String {
        format!("_{}_", Self::escape_slack(text))
    }

    fn format_link(&self, text: &str, url: &str) -> String {
        format!("<{}|{}>", url, Self::escape_slack(text))
    }

    fn format_mention(&self, user_id: &str) -> String {
        format!("<@{user_id}>")
    }

    fn escape(&self, text: &str) -> String {
        Self::escape_slack(text)
    }
}

/// Plain text formatter (strips all formatting)
pub struct PlainTextFormatter;

impl PlainTextFormatter {
    /// Create a new formatter
    pub fn new() -> Self {
        Self
    }

    /// Strip all markdown formatting
    pub fn strip_markdown(text: &str) -> String {
        let mut result = text.to_string();

        // Code blocks
        result = regex::Regex::new(r"```[\w]*\n(.*?)```")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();

        // Bold
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();

        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();

        // Italic: remove *text* and _text_
        // Process bold first to protect, then italic, then restore bold
        let bold_placeholder = "\x00BOLD\x00";
        result = regex::Regex::new(r"\*\*(.+?)\*\*")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        result = regex::Regex::new(r"__(.+?)__")
            .unwrap()
            .replace_all(&result, |caps: &regex::Captures<'_>| {
                format!("{}{}{}", bold_placeholder, caps.get(1).map(|m| m.as_str()).unwrap_or(""), bold_placeholder)
            })
            .to_string();

        // Now safe to process italic
        result = regex::Regex::new(r"\*([^*]+)\*")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();

        result = regex::Regex::new(r"_([^_]+)_")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();

        // Restore bold placeholders
        result = result.replace(bold_placeholder, "");

        // Inline code
        result = regex::Regex::new(r"`([^`]+)`")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();

        // Strikethrough
        result = regex::Regex::new(r"~~(.+?)~~")
            .unwrap()
            .replace_all(&result, "$1")
            .to_string();

        // Links: keep text, add URL in parentheses
        result = regex::Regex::new(r"\[([^\]]+)\]\(([^)]+)\)")
            .unwrap()
            .replace_all(&result, "$1 ($2)")
            .to_string();

        result
    }
}

impl Default for PlainTextFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageFormatter for PlainTextFormatter {
    fn format(&self, text: &str) -> String {
        Self::strip_markdown(text)
    }

    fn format_code_block(&self, code: &str, _language: Option<&str>) -> String {
        code.to_string()
    }

    fn format_inline_code(&self, code: &str) -> String {
        code.to_string()
    }

    fn format_bold(&self, text: &str) -> String {
        text.to_string()
    }

    fn format_italic(&self, text: &str) -> String {
        text.to_string()
    }

    fn format_link(&self, text: &str, url: &str) -> String {
        format!("{text} ({url})")
    }

    fn format_mention(&self, user_id: &str) -> String {
        format!("@{user_id}")
    }

    fn escape(&self, text: &str) -> String {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telegram_formatter() {
        let formatter = TelegramHtmlFormatter::new();

        assert_eq!(
            formatter.format_bold("test"),
            "<b>test</b>"
        );
        assert_eq!(
            formatter.format_italic("test"),
            "<i>test</i>"
        );
        assert_eq!(
            formatter.format_inline_code("test"),
            "<code>test</code>"
        );

        let md = "**bold** and *italic*";
        let html = formatter.format(md);
        assert!(html.contains("<b>bold</b>"));
        assert!(html.contains("<i>italic</i>"));
    }

    #[test]
    fn test_discord_formatter() {
        let formatter = DiscordFormatter::new();

        assert_eq!(
            formatter.format_bold("test"),
            "**test**"
        );
        assert_eq!(
            formatter.format_italic("test"),
            "*test*"
        );

        let md = "**bold** and *italic*";
        let formatted = formatter.format(md);
        assert!(formatted.contains("**bold**"));
        assert!(formatted.contains("*italic*"));
    }

    #[test]
    fn test_slack_formatter() {
        let formatter = SlackFormatter::new();

        assert_eq!(
            formatter.format_bold("test"),
            "*test*"
        );
        assert_eq!(
            formatter.format_italic("test"),
            "_test_"
        );

        let md = "**bold** and *italic*";
        let mrkdwn = formatter.format(md);
        assert!(mrkdwn.contains("*bold*")); // Slack bold is single asterisk
        assert!(mrkdwn.contains("_italic_"));
    }

    #[test]
    fn test_plain_text_formatter() {
        let formatter = PlainTextFormatter::new();

        let md = "**bold** and [link](http://example.com)";
        let plain = formatter.format(md);
        assert_eq!(plain, "bold and link (http://example.com)");
    }
}
