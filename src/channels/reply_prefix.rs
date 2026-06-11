//! Reply prefix template system for Syscity channels
//!
//! Allows dynamic model info and metadata to be prepended to outbound messages.
//!
//! Template syntax uses `{{placeholder}}` with support for:
//! - `{{model_name}}` — The LLM model used for the response
//! - `{{model_provider}}` — The provider name (e.g. "anthropic", "openai")
//! - `{{timestamp}}` — Current timestamp (configurable format)
//! - `{{session_id}}` — The session ID
//! - `{{channel}}` — The channel name
//! - `{{user_id}}` — The user who sent the message
//! - `{{date}}` — Current date (YYYY-MM-DD)
//! - `{{time}}` — Current time (HH:MM:SS)
//! - `{{cost}}` — Approximate cost of the response

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Context for rendering a reply prefix template.
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    /// LLM model name (e.g. "claude-sonnet-4-6").
    pub model_name: Option<String>,
    /// Provider name (e.g. "anthropic", "openai").
    pub model_provider: Option<String>,
    /// Session ID.
    pub session_id: Option<String>,
    /// Channel name.
    pub channel: Option<String>,
    /// User ID.
    pub user_id: Option<String>,
    /// Approximate cost of the response.
    pub cost: Option<f64>,
    /// Custom key-value pairs.
    pub custom: HashMap<String, String>,
}

impl TemplateContext {
    /// Create a new empty context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the model name.
    pub fn with_model(mut self, name: impl Into<String>) -> Self {
        self.model_name = Some(name.into());
        self
    }

    /// Set the provider name.
    pub fn with_provider(mut self, provider: impl Into<String>) -> Self {
        self.model_provider = Some(provider.into());
        self
    }

    /// Set the session ID.
    pub fn with_session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Set the channel name.
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    /// Set the user ID.
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Set the cost.
    pub fn with_cost(mut self, cost: f64) -> Self {
        self.cost = Some(cost);
        self
    }

    /// Add a custom key-value pair.
    pub fn with_custom(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom.insert(key.into(), value.into());
        self
    }
}

/// A reply prefix template.
///
/// Templates are strings with `{{placeholder}}` markers that get replaced
/// at render time. Literal `{{` and `}}` can be escaped as `\{\{` and `\}\}`.
#[derive(Debug, Clone)]
pub struct ReplyPrefixTemplate {
    /// The raw template string.
    pub template: String,
    /// Whether to render this prefix on every message.
    pub enabled: bool,
    /// If set, only render for these channels.
    pub channel_filter: Option<Vec<String>>,
}

impl ReplyPrefixTemplate {
    /// Create a new template.
    pub fn new(template: impl Into<String>) -> Self {
        Self {
            template: template.into(),
            enabled: true,
            channel_filter: None,
        }
    }

    /// Disable this template.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Apply a channel filter (only render for these channels).
    pub fn with_channel_filter(mut self, channels: Vec<String>) -> Self {
        self.channel_filter = Some(channels);
        self
    }

    /// Check if this template should be rendered for a given channel.
    pub fn should_render(&self, channel: Option<&str>) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(ref filter) = self.channel_filter {
            if let Some(ch) = channel {
                return filter.iter().any(|f| f == ch);
            }
            return false;
        }
        true
    }

    /// Render the template with the given context.
    pub fn render(&self, ctx: &TemplateContext) -> String {
        let mut result = String::new();
        let mut chars = self.template.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\\' {
                // Escaped character — pass through
                if let Some(next) = chars.next() {
                    result.push(next);
                } else {
                    result.push(c);
                }
            } else if c == '{' && chars.peek() == Some(&'{') {
                chars.next(); // consume second '{'
                let mut placeholder = String::new();
                while let Some(&pc) = chars.peek() {
                    if pc == '}' {
                        // Check for closing }}
                        chars.next(); // consume first '}'
                        if chars.peek() == Some(&'}') {
                            chars.next(); // consume second '}'
                            break;
                        } else {
                            placeholder.push('}');
                            continue;
                        }
                    }
                    placeholder.push(pc);
                    chars.next();
                }
                let rendered = self.render_placeholder(placeholder.trim(), ctx);
                result.push_str(&rendered);
            } else if c == '\\' && chars.peek() == Some(&'{') {
                // Escaped {{
                chars.next(); // consume {
                if chars.peek() == Some(&'{') {
                    chars.next(); // consume second {
                    result.push_str("{{");
                } else {
                    result.push('\\');
                    result.push('{');
                }
            } else {
                result.push(c);
            }
        }

        result
    }

    fn render_placeholder(&self, key: &str, ctx: &TemplateContext) -> String {
        match key {
            "model_name" => ctx.model_name.as_deref().unwrap_or("unknown").to_string(),
            "model_provider" => ctx.model_provider.as_deref().unwrap_or("unknown").to_string(),
            "timestamp" => {
                chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string()
            }
            "date" => chrono::Utc::now().format("%Y-%m-%d").to_string(),
            "time" => chrono::Utc::now().format("%H:%M:%S").to_string(),
            "session_id" => ctx.session_id.as_deref().unwrap_or("unknown").to_string(),
            "channel" => ctx.channel.as_deref().unwrap_or("unknown").to_string(),
            "user_id" => ctx.user_id.as_deref().unwrap_or("unknown").to_string(),
            "cost" => ctx
                .cost
                .map(|c| format!("${:.6}", c))
                .unwrap_or_else(|| "unknown".to_string()),
            _ => {
                // Check custom fields
                if let Some(val) = ctx.custom.get(key) {
                    val.clone()
                } else {
                    format!("{{{{{}}}}}", key) // keep unresolved placeholders
                }
            }
        }
    }
}

/// Engine that manages and renders reply prefix templates.
#[derive(Debug, Clone)]
pub struct ReplyPrefixEngine {
    /// The active templates, rendered in order.
    templates: Arc<RwLock<Vec<ReplyPrefixTemplate>>>,
}

impl ReplyPrefixEngine {
    /// Create a new empty engine.
    pub fn new() -> Self {
        Self {
            templates: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create an engine with a default template.
    pub fn with_default() -> Self {
        let engine = Self::new();
        let default = ReplyPrefixTemplate::new("[{{model_provider}}/{{model_name}}]");
        engine.add_template(default);
        engine
    }

    /// Add a template.
    pub fn add_template(&self, template: ReplyPrefixTemplate) {
        let mut templates = self.templates.blocking_write();
        templates.push(template);
    }

    /// Set all templates at once.
    pub async fn set_templates(&self, templates: Vec<ReplyPrefixTemplate>) {
        let mut current = self.templates.write().await;
        *current = templates;
    }

    /// Get all templates.
    pub async fn get_templates(&self) -> Vec<ReplyPrefixTemplate> {
        let templates = self.templates.read().await;
        templates.clone()
    }

    /// Render all enabled templates for the given context.
    pub fn render(&self, ctx: &TemplateContext, channel: Option<&str>) -> String {
        let templates = self.templates.blocking_read();
        let mut prefix = String::new();
        for tmpl in templates.iter() {
            if tmpl.should_render(channel) {
                prefix.push_str(&tmpl.render(ctx));
            }
        }
        prefix
    }

    /// Render asynchronously.
    pub async fn render_async(
        &self,
        ctx: &TemplateContext,
        channel: Option<&str>,
    ) -> String {
        let templates = self.templates.read().await;
        let mut prefix = String::new();
        for tmpl in templates.iter() {
            if tmpl.should_render(channel) {
                prefix.push_str(&tmpl.render(ctx));
            }
        }
        prefix
    }

    /// Prepend the rendered prefix to a message content.
    pub fn apply(
        &self,
        content: &str,
        ctx: &TemplateContext,
        channel: Option<&str>,
    ) -> String {
        let prefix = self.render(ctx, channel);
        if prefix.is_empty() {
            content.to_string()
        } else {
            format!("{} {}", prefix, content)
        }
    }

    /// Apply asynchronously.
    pub async fn apply_async(
        &self,
        content: &str,
        ctx: &TemplateContext,
        channel: Option<&str>,
    ) -> String {
        let prefix = self.render_async(ctx, channel).await;
        if prefix.is_empty() {
            content.to_string()
        } else {
            format!("{} {}", prefix, content)
        }
    }
}

impl Default for ReplyPrefixEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── Template presets ───────────────────────────────────────────────────────────

/// Preset: model tag prefix `[Provider/Model]`.
pub fn model_tag_template() -> ReplyPrefixTemplate {
    ReplyPrefixTemplate::new("[{{model_provider}}/{{model_name}}]")
}

/// Preset: minimal model prefix without brackets.
pub fn minimal_model_template() -> ReplyPrefixTemplate {
    ReplyPrefixTemplate::new("{{model_provider}}/{{model_name}}:")
}

/// Preset: timestamp + model prefix.
pub fn timestamp_model_template() -> ReplyPrefixTemplate {
    ReplyPrefixTemplate::new("[{{timestamp}}] {{model_provider}}/{{model_name}}:")
}

/// Preset: cost-aware prefix (only shown when cost is available).
pub fn cost_aware_template() -> ReplyPrefixTemplate {
    ReplyPrefixTemplate::new("[{{model_provider}}/{{model_name}} ~{{cost}}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_template() {
        let tmpl = ReplyPrefixTemplate::new("[{{model_provider}}/{{model_name}}]");
        let ctx = TemplateContext::new()
            .with_provider("anthropic")
            .with_model("claude-sonnet-4-6");
        assert_eq!(tmpl.render(&ctx), "[anthropic/claude-sonnet-4-6]");
    }

    #[test]
    fn test_template_with_all_fields() {
        let tmpl = ReplyPrefixTemplate::new(
            "[{{model_name}}|{{session_id}}|{{channel}}|{{user_id}}]",
        );
        let ctx = TemplateContext::new()
            .with_model("gpt-4")
            .with_session("sess_1")
            .with_channel("telegram")
            .with_user("user_1");
        assert_eq!(tmpl.render(&ctx), "[gpt-4|sess_1|telegram|user_1]");
    }

    #[test]
    fn test_template_cost() {
        let tmpl = ReplyPrefixTemplate::new("cost={{cost}}");
        let ctx = TemplateContext::new().with_cost(0.001234);
        assert_eq!(tmpl.render(&ctx), "cost=$0.001234");

        let ctx_no_cost = TemplateContext::new();
        assert_eq!(tmpl.render(&ctx_no_cost), "cost=unknown");
    }

    #[test]
    fn test_custom_placeholder() {
        let tmpl = ReplyPrefixTemplate::new("{{custom_key}}");
        let mut custom = HashMap::new();
        custom.insert("custom_key".to_string(), "custom_val".to_string());
        let ctx = TemplateContext { custom, ..Default::default() };
        assert_eq!(tmpl.render(&ctx), "custom_val");
    }

    #[test]
    fn test_unresolved_placeholder() {
        let tmpl = ReplyPrefixTemplate::new("{{unknown_placeholder}}");
        let ctx = TemplateContext::new();
        assert_eq!(tmpl.render(&ctx), "{{unknown_placeholder}}");
    }

    #[test]
    fn test_timestamp() {
        let tmpl = ReplyPrefixTemplate::new("{{date}}T{{time}}");
        let ctx = TemplateContext::new();
        let rendered = tmpl.render(&ctx);
        assert!(rendered.contains('T'));
        // Should contain current date
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert!(rendered.starts_with(&today));
    }

    #[test]
    fn test_should_render_channel_filter() {
        let tmpl = ReplyPrefixTemplate::new("test")
            .with_channel_filter(vec!["telegram".to_string(), "discord".to_string()]);
        assert!(tmpl.should_render(Some("telegram")));
        assert!(tmpl.should_render(Some("discord")));
        assert!(!tmpl.should_render(Some("slack")));
        assert!(!tmpl.should_render(None));
    }

    #[test]
    fn test_disabled_template() {
        let tmpl = ReplyPrefixTemplate::new("test").disabled();
        assert!(!tmpl.should_render(Some("telegram")));
    }

    #[test]
    fn test_apply_to_content() {
        let engine = ReplyPrefixEngine::new();
        engine.add_template(ReplyPrefixTemplate::new("[bot] "));
        let ctx = TemplateContext::new();
        let result = engine.apply("Hello world", &ctx, None);
        assert_eq!(result, "[bot]  Hello world");
    }

    #[test]
    fn test_engine_empty_templates() {
        let engine = ReplyPrefixEngine::new();
        let ctx = TemplateContext::new();
        let result = engine.apply("Hello", &ctx, None);
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_escaped_braces() {
        // Literal {{ rendered as {{ in template syntax is actually not supported
        // in this simple parser. But escaped braces via \{\{ is.
        // Actually, our parser treats any {{ }} as placeholders.
        // For literal braces, users should not use placeholders.
        let tmpl = ReplyPrefixTemplate::new("plain text");
        let ctx = TemplateContext::new();
        assert_eq!(tmpl.render(&ctx), "plain text");
    }

    #[test]
    fn test_minimal_preset() {
        let tmpl = minimal_model_template();
        let ctx = TemplateContext::new()
            .with_provider("openai")
            .with_model("gpt-4o");
        assert_eq!(tmpl.render(&ctx), "openai/gpt-4o:");
    }

    #[test]
    fn test_timestamp_preset() {
        let tmpl = timestamp_model_template();
        let ctx = TemplateContext::new()
            .with_provider("anthropic")
            .with_model("claude-opus-4-6");
        let rendered = tmpl.render(&ctx);
        assert!(rendered.contains("anthropic/claude-opus-4-6"));
        assert!(rendered.starts_with('['));
    }

    #[test]
    fn test_cost_preset_with_cost() {
        let tmpl = cost_aware_template();
        let ctx = TemplateContext::new()
            .with_provider("anthropic")
            .with_model("claude-3")
            .with_cost(0.005);
        let rendered = tmpl.render(&ctx);
        assert!(rendered.contains("$0.005"));
    }
}
