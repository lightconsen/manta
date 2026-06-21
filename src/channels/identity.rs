//! Sender identity validation for Syscity channels
//!
//! Provides structured identity types and validators supporting:
//! - E164 phone number format (`+\d{3,}`)
//! - Username validation (alphanumeric, length, allowed chars)
//! - Multi-field identity (user_id, username, phone, email, display_name)
//! - Per-platform identity types (Telegram, Discord, Slack, etc.)

use std::fmt;

use serde::{Deserialize, Serialize};

/// A validated sender identity with multiple possible identity fields.
///
/// At minimum `user_id` must be present; all other fields are optional
/// and channel-specific.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SenderIdentity {
    /// Primary user ID (always present, channel-specific format).
    pub user_id: String,
    /// Display name (may contain Unicode, emoji, spaces).
    pub display_name: Option<String>,
    /// Username / handle (e.g. `@alice` without the `@`).
    pub username: Option<String>,
    /// Phone number in E164 format (e.g. `+8613800138000`).
    pub phone: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// Channel-specific raw identity data (e.g. Discord `user#1234`).
    pub raw: Option<String>,
    /// Platform-specific metadata.
    pub platform_data: Option<serde_json::Value>,
}

impl SenderIdentity {
    /// Create a new identity with just a user ID.
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            display_name: None,
            username: None,
            phone: None,
            email: None,
            raw: None,
            platform_data: None,
        }
    }

    /// Set the display name.
    pub fn with_display_name(mut self, name: impl Into<String>) -> Self {
        self.display_name = Some(name.into());
        self
    }

    /// Set the username.
    pub fn with_username(mut self, username: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self
    }

    /// Set the phone number.
    pub fn with_phone(mut self, phone: impl Into<String>) -> Self {
        self.phone = Some(phone.into());
        self
    }

    /// Set the email address.
    pub fn with_email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    /// Set raw identity string (channel-specific).
    pub fn with_raw(mut self, raw: impl Into<String>) -> Self {
        self.raw = Some(raw.into());
        self
    }

    /// Set platform-specific metadata.
    pub fn with_platform_data(mut self, data: serde_json::Value) -> Self {
        self.platform_data = Some(data);
        self
    }

    /// Return the best display name: display_name > username > user_id.
    pub fn display_name(&self) -> &str {
        self.display_name
            .as_deref()
            .or(self.username.as_deref())
            .unwrap_or(&self.user_id)
    }

    /// Return true if this identity has any contact info beyond the user ID.
    pub fn has_contact_info(&self) -> bool {
        self.display_name.is_some()
            || self.username.is_some()
            || self.phone.is_some()
            || self.email.is_some()
    }
}

impl fmt::Display for SenderIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// ── Validation
// ─────────────────────────────────────────────────────────────────

/// Errors that can occur during identity validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityValidationError {
    /// User ID is empty.
    EmptyUserId,
    /// User ID contains invalid characters.
    InvalidUserId(String),
    /// User ID exceeds maximum length.
    UserIdTooLong { max: usize, actual: usize },
    /// Invalid E164 phone number format.
    InvalidE164Phone(String),
    /// Phone number missing or empty when required.
    MissingPhone,
    /// Username contains invalid characters.
    InvalidUsername(String),
    /// Username too short.
    UsernameTooShort { min: usize, actual: usize },
    /// Username too long.
    UsernameTooLong { max: usize, actual: usize },
    /// Invalid email format.
    InvalidEmail(String),
    /// Display name contains invalid content.
    InvalidDisplayName(String),
}

impl fmt::Display for IdentityValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyUserId => write!(f, "User ID must not be empty"),
            Self::InvalidUserId(s) => write!(f, "Invalid user ID: {}", s),
            Self::UserIdTooLong { max, actual } => {
                write!(f, "User ID too long: {} chars (max {})", actual, max)
            }
            Self::InvalidE164Phone(s) => write!(f, "Invalid E164 phone number: {}", s),
            Self::MissingPhone => write!(f, "Phone number is required"),
            Self::InvalidUsername(s) => write!(f, "Invalid username: {}", s),
            Self::UsernameTooShort { min, actual } => {
                write!(f, "Username too short: {} chars (min {})", actual, min)
            }
            Self::UsernameTooLong { max, actual } => {
                write!(f, "Username too long: {} chars (max {})", actual, max)
            }
            Self::InvalidEmail(s) => write!(f, "Invalid email: {}", s),
            Self::InvalidDisplayName(s) => write!(f, "Invalid display name: {}", s),
        }
    }
}

impl std::error::Error for IdentityValidationError {}

/// Validation result for a single identity field.
pub type IdentityValidationResult<T = ()> = Result<T, IdentityValidationError>;

/// Configuration for identity validation rules.
#[derive(Debug, Clone)]
pub struct IdentityValidatorConfig {
    /// Maximum length for user IDs (default: 128).
    pub max_user_id_length: usize,
    /// Allowed characters pattern for user IDs (default: alphanumeric +
    /// `_-./@:`).
    pub user_id_allowed_chars: AllowedCharSet,
    /// Minimum username length (default: 2).
    pub min_username_length: usize,
    /// Maximum username length (default: 32).
    pub max_username_length: usize,
    /// Whether phone is required.
    pub require_phone: bool,
    /// Whether email format is validated.
    pub validate_email: bool,
    /// Maximum display name length (default: 100).
    pub max_display_name_length: usize,
}

impl Default for IdentityValidatorConfig {
    fn default() -> Self {
        Self {
            max_user_id_length: 128,
            user_id_allowed_chars: AllowedCharSet::Default,
            min_username_length: 2,
            max_username_length: 32,
            require_phone: false,
            validate_email: false,
            max_display_name_length: 100,
        }
    }
}

/// Character set allowed in user IDs.
#[derive(Debug, Clone)]
pub enum AllowedCharSet {
    /// Alphanumeric plus `_-./@:` (default).
    Default,
    /// Only alphanumeric (a-z, A-Z, 0-9).
    AlphanumericOnly,
    /// Telegram-specific: alphanumeric + `_`.
    Telegram,
    /// Discord-specific: alphanumeric + `_-.`.
    Discord,
    /// Custom regex pattern.
    Custom(String),
}

impl AllowedCharSet {
    /// Check if a character is allowed by this set.
    pub fn allows(&self, c: char) -> bool {
        match self {
            Self::Default => c.is_alphanumeric() || "_-./@:".contains(c),
            Self::AlphanumericOnly => c.is_alphanumeric(),
            Self::Telegram => c.is_alphanumeric() || c == '_',
            Self::Discord => c.is_alphanumeric() || "_-.".contains(c),
            Self::Custom(_pattern) => {
                // For custom patterns, we validate at the string level
                // (fall through — checked by contains_match)
                true
            }
        }
    }

    /// Check if the entire string matches this character set.
    pub fn contains_match(&self, s: &str) -> bool {
        match self {
            Self::Custom(pattern) => regex::Regex::new(pattern)
                .map(|re| re.is_match(s))
                .unwrap_or(false),
            _ => s.chars().all(|c| self.allows(c)),
        }
    }
}

/// Identity validator with configurable rules.
#[derive(Debug, Clone)]
pub struct IdentityValidator {
    config: IdentityValidatorConfig,
}

impl IdentityValidator {
    /// Create a new validator with default config.
    pub fn new() -> Self {
        Self::with_config(IdentityValidatorConfig::default())
    }

    /// Create a validator with custom config.
    pub fn with_config(config: IdentityValidatorConfig) -> Self {
        Self { config }
    }

    /// Create a validator permissive enough for any user ID format.
    pub fn permissive() -> Self {
        Self::with_config(IdentityValidatorConfig {
            max_user_id_length: 256,
            user_id_allowed_chars: AllowedCharSet::Default,
            ..Default::default()
        })
    }

    /// Validate a full `SenderIdentity`.
    pub fn validate(&self, identity: &SenderIdentity) -> IdentityValidationResult {
        self.validate_user_id(&identity.user_id)?;

        if let Some(ref username) = identity.username {
            self.validate_username(username)?;
        }

        if let Some(ref phone) = identity.phone {
            self.validate_phone(phone)?;
        } else if self.config.require_phone {
            return Err(IdentityValidationError::MissingPhone);
        }

        if let Some(ref email) = identity.email {
            if self.config.validate_email {
                self.validate_email(email)?;
            }
        }

        if let Some(ref display) = identity.display_name {
            if display.chars().count() > self.config.max_display_name_length {
                return Err(IdentityValidationError::InvalidDisplayName(format!(
                    "Display name too long: {} chars (max {})",
                    display.chars().count(),
                    self.config.max_display_name_length
                )));
            }
        }

        Ok(())
    }

    /// Validate a user ID.
    pub fn validate_user_id(&self, user_id: &str) -> IdentityValidationResult {
        if user_id.is_empty() {
            return Err(IdentityValidationError::EmptyUserId);
        }
        if user_id.chars().count() > self.config.max_user_id_length {
            return Err(IdentityValidationError::UserIdTooLong {
                max: self.config.max_user_id_length,
                actual: user_id.chars().count(),
            });
        }
        if !self.config.user_id_allowed_chars.contains_match(user_id) {
            return Err(IdentityValidationError::InvalidUserId(
                "User ID contains disallowed characters".to_string(),
            ));
        }
        Ok(())
    }

    /// Validate a username.
    pub fn validate_username(&self, username: &str) -> IdentityValidationResult {
        if username.is_empty() {
            return Err(IdentityValidationError::InvalidUsername(
                "Username must not be empty".to_string(),
            ));
        }
        let len = username.chars().count();
        if len < self.config.min_username_length {
            return Err(IdentityValidationError::UsernameTooShort {
                min: self.config.min_username_length,
                actual: len,
            });
        }
        if len > self.config.max_username_length {
            return Err(IdentityValidationError::UsernameTooLong {
                max: self.config.max_username_length,
                actual: len,
            });
        }
        if !username
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
        {
            return Err(IdentityValidationError::InvalidUsername(
                "Username may only contain alphanumeric characters, underscores, hyphens, and dots"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Validate an E164 phone number.
    ///
    /// E164 format: `+` followed by 3-15 digits.
    pub fn validate_phone(&self, phone: &str) -> IdentityValidationResult {
        if phone.is_empty() {
            return Err(IdentityValidationError::InvalidE164Phone(
                "Phone must not be empty".to_string(),
            ));
        }
        if !phone.starts_with('+') {
            return Err(IdentityValidationError::InvalidE164Phone(
                "Phone must start with '+'".to_string(),
            ));
        }
        let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() != phone.len() - 1 {
            return Err(IdentityValidationError::InvalidE164Phone(
                "Phone may only contain digits after '+'".to_string(),
            ));
        }
        if digits.len() < 3 {
            return Err(IdentityValidationError::InvalidE164Phone(
                "Phone must have at least 3 digits".to_string(),
            ));
        }
        if digits.len() > 15 {
            return Err(IdentityValidationError::InvalidE164Phone(
                "Phone must have at most 15 digits".to_string(),
            ));
        }
        Ok(())
    }

    /// Validate an email address (basic format check).
    pub fn validate_email(&self, email: &str) -> IdentityValidationResult {
        if email.is_empty() {
            return Err(IdentityValidationError::InvalidEmail(
                "Email must not be empty".to_string(),
            ));
        }
        let parts: Vec<&str> = email.splitn(2, '@').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(IdentityValidationError::InvalidEmail(
                "Email must contain exactly one '@' with non-empty local and domain parts"
                    .to_string(),
            ));
        }
        if !parts[1].contains('.') {
            return Err(IdentityValidationError::InvalidEmail(
                "Email domain must contain a dot".to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for IdentityValidator {
    fn default() -> Self {
        Self::new()
    }
}

// ── Platform-specific helpers
// ──────────────────────────────────────────────────

/// Build a `SenderIdentity` from a Telegram user.
pub fn telegram_identity(
    user_id: i64,
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
) -> SenderIdentity {
    let display_name = match (first_name, last_name) {
        (Some(f), Some(l)) => Some(format!("{} {}", f, l)),
        (Some(f), None) => Some(f.to_string()),
        _ => None,
    };
    SenderIdentity::new(user_id.to_string())
        .with_username(username.unwrap_or_default())
        .with_display_name(display_name.unwrap_or_default())
}

/// Build a `SenderIdentity` from a Discord user.
pub fn discord_identity(
    user_id: &str,
    global_name: Option<&str>,
    username: Option<&str>,
) -> SenderIdentity {
    let display_name = global_name.or(username).unwrap_or(user_id);
    SenderIdentity::new(user_id.to_string())
        .with_username(username.unwrap_or_default())
        .with_display_name(display_name)
        .with_platform_data(serde_json::json!({
            "discord_id": user_id,
        }))
}

/// Build a `SenderIdentity` from a Slack user.
pub fn slack_identity(
    user_id: &str,
    real_name: Option<&str>,
    display_name: Option<&str>,
) -> SenderIdentity {
    SenderIdentity::new(user_id.to_string())
        .with_display_name(real_name.or(display_name).unwrap_or(user_id))
        .with_username(display_name.unwrap_or(""))
        .with_raw(user_id.to_string())
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sender_identity_new() {
        let id = SenderIdentity::new("u123");
        assert_eq!(id.user_id, "u123");
        assert_eq!(id.display_name(), "u123");
        assert!(!id.has_contact_info());
    }

    #[test]
    fn test_sender_identity_with_fields() {
        let id = SenderIdentity::new("u123")
            .with_display_name("Alice")
            .with_username("alice99")
            .with_phone("+8613800138000");
        assert_eq!(id.display_name(), "Alice");
        assert!(id.has_contact_info());
    }

    #[test]
    fn test_display_name_fallback() {
        let id = SenderIdentity::new("u456").with_username("bob");
        assert_eq!(id.display_name(), "bob");

        let id = SenderIdentity::new("u789");
        assert_eq!(id.display_name(), "u789");
    }

    // ── Validator tests ────────────────────────────────────────────────

    #[test]
    fn test_validate_empty_user_id() {
        let validator = IdentityValidator::new();
        assert_eq!(validator.validate_user_id(""), Err(IdentityValidationError::EmptyUserId));
    }

    #[test]
    fn test_validate_user_id_too_long() {
        let validator = IdentityValidator::new();
        let long_id = "a".repeat(200);
        assert!(matches!(
            validator.validate_user_id(&long_id),
            Err(IdentityValidationError::UserIdTooLong { .. })
        ));
    }

    #[test]
    fn test_validate_user_id_allowed_chars() {
        let validator = IdentityValidator::new();
        assert!(validator.validate_user_id("user_123").is_ok());
        assert!(validator.validate_user_id("user.name@domain").is_ok());
    }

    #[test]
    fn test_validate_e164_phone_valid() {
        let validator = IdentityValidator::new();
        assert!(validator.validate_phone("+8613800138000").is_ok());
        assert!(validator.validate_phone("+12025551234").is_ok());
        assert!(validator.validate_phone("+447911123456").is_ok());
    }

    #[test]
    fn test_validate_e164_phone_missing_plus() {
        let validator = IdentityValidator::new();
        assert_eq!(
            validator.validate_phone("8613800138000"),
            Err(IdentityValidationError::InvalidE164Phone(
                "Phone must start with '+'".to_string()
            ))
        );
    }

    #[test]
    fn test_validate_e164_phone_too_short() {
        let validator = IdentityValidator::new();
        assert_eq!(
            validator.validate_phone("+12"),
            Err(IdentityValidationError::InvalidE164Phone(
                "Phone must have at least 3 digits".to_string()
            ))
        );
    }

    #[test]
    fn test_validate_username_short() {
        let validator = IdentityValidator::new();
        assert_eq!(
            validator.validate_username("a"),
            Err(IdentityValidationError::UsernameTooShort { min: 2, actual: 1 })
        );
    }

    #[test]
    fn test_validate_username_valid() {
        let validator = IdentityValidator::new();
        assert!(validator.validate_username("alice_99").is_ok());
        assert!(validator.validate_username("bob").is_ok());
    }

    #[test]
    fn test_validate_username_invalid_chars() {
        let validator = IdentityValidator::new();
        assert!(matches!(
            validator.validate_username("alice@user"),
            Err(IdentityValidationError::InvalidUsername(_))
        ));
    }

    #[test]
    fn test_validate_email_basic() {
        let validator = IdentityValidator::new();
        assert!(validator.validate_email("user@example.com").is_ok());
        assert_eq!(
            validator.validate_email("not-an-email"),
            Err(IdentityValidationError::InvalidEmail(
                "Email must contain exactly one '@' with non-empty local and domain parts"
                    .to_string()
            ))
        );
    }

    #[test]
    fn test_validate_full_identity() {
        let validator = IdentityValidator::new();
        let id = SenderIdentity::new("u123")
            .with_username("alice")
            .with_phone("+12025551234");
        assert!(validator.validate(&id).is_ok());
    }

    #[test]
    fn test_validate_identity_with_bad_phone() {
        let validator = IdentityValidator::new();
        let id = SenderIdentity::new("u123")
            .with_username("alice")
            .with_phone("not-a-phone");
        assert!(validator.validate(&id).is_err());
    }

    #[test]
    fn test_telegram_identity_builder() {
        let id = telegram_identity(12345, Some("alice_bot"), Some("Alice"), Some("Smith"));
        assert_eq!(id.display_name(), "Alice Smith");
        assert_eq!(id.user_id, "12345");
        assert_eq!(id.username, Some("alice_bot".to_string()));
    }

    #[test]
    fn test_discord_identity_builder() {
        let id = discord_identity("123456789", Some("Alice"), Some("alice#1234"));
        assert_eq!(id.user_id, "123456789");
        assert_eq!(id.display_name(), "Alice");
        assert!(id.platform_data.is_some());
    }

    #[test]
    fn test_slack_identity_builder() {
        let id = slack_identity("U12345", Some("Alice Smith"), Some("alice"));
        assert_eq!(id.display_name(), "Alice Smith");
        assert_eq!(id.raw, Some("U12345".to_string()));
    }

    #[test]
    fn test_allowed_charset_telegram() {
        let set = AllowedCharSet::Telegram;
        assert!(set.contains_match("alice_99"));
        assert!(!set.contains_match("alice.99")); // dot not allowed in telegram
    }
}
