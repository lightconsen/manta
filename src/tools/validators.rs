//! Tool validation: name, schema, and security checks.

use serde_json::Value;

use super::Tool;

/// Trait for custom tool validators
pub trait ToolValidator: Send + Sync + std::fmt::Debug {
    /// Validate a tool before registration
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError>;
    /// Validate tool input arguments
    fn validate_input(&self, tool_name: &str, args: &Value) -> Result<(), ToolValidationError>;
}

/// Tool validation errors
#[derive(Debug, Clone)]
pub enum ToolValidationError {
    /// Invalid tool name
    InvalidName(String),
    /// Invalid schema
    InvalidSchema(String),
    /// Input validation failed
    InvalidInput(String),
    /// Security violation
    SecurityViolation(String),
}

impl std::fmt::Display for ToolValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(s) => write!(f, "Invalid tool name: {}", s),
            Self::InvalidSchema(s) => write!(f, "Invalid tool schema: {}", s),
            Self::InvalidInput(s) => write!(f, "Invalid tool input: {}", s),
            Self::SecurityViolation(s) => write!(f, "Security violation: {}", s),
        }
    }
}

impl std::error::Error for ToolValidationError {}

/// Name validator - ensures tool names follow conventions
#[derive(Debug)]
pub struct NameValidator;

impl ToolValidator for NameValidator {
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError> {
        let name = tool.name();

        // Check length
        if name.len() < 2 || name.len() > 64 {
            return Err(ToolValidationError::InvalidName(format!(
                "Tool name '{}' must be between 2 and 64 characters",
                name
            )));
        }

        // Check characters (alphanumeric, underscore, hyphen only)
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(ToolValidationError::InvalidName(format!(
                "Tool name '{}' contains invalid characters. Use alphanumeric, underscore, or \
                 hyphen only",
                name
            )));
        }

        // Check doesn't start with number
        if name.chars().next().map(|c| c.is_numeric()).unwrap_or(false) {
            return Err(ToolValidationError::InvalidName(format!(
                "Tool name '{}' cannot start with a number",
                name
            )));
        }

        Ok(())
    }

    fn validate_input(&self, _tool_name: &str, _args: &Value) -> Result<(), ToolValidationError> {
        Ok(())
    }
}

/// Schema validator - validates JSON schemas
#[derive(Debug)]
pub struct SchemaValidator;

impl ToolValidator for SchemaValidator {
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError> {
        let schema = tool.parameters_schema();

        // Check schema has required fields
        if !schema.get("type").map(|v| v == "object").unwrap_or(false) {
            return Err(ToolValidationError::InvalidSchema(
                "Schema must have type 'object'".to_string(),
            ));
        }

        if schema.get("properties").is_none() {
            return Err(ToolValidationError::InvalidSchema(
                "Schema must have 'properties' field".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_input(&self, tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        // Basic JSON structure validation
        if !args.is_object() && !args.is_null() {
            return Err(ToolValidationError::InvalidInput(format!(
                "Tool '{}' arguments must be a JSON object",
                tool_name
            )));
        }

        Ok(())
    }
}

/// Security validator - checks for dangerous patterns
#[derive(Debug)]
pub struct SecurityValidator;

impl SecurityValidator {
    /// Check for path traversal attempts
    pub(super) fn check_path_traversal(&self, value: &str) -> Result<(), ToolValidationError> {
        let dangerous_patterns = ["../", "..\\", "~/..", "/..", "%2e%2e%2f", "%252e%252e%252f"];

        for pattern in &dangerous_patterns {
            if value.contains(pattern) {
                return Err(ToolValidationError::SecurityViolation(format!(
                    "Path traversal attempt detected: {}",
                    pattern
                )));
            }
        }

        // Check for double slashes (can be used in some path traversal attacks)
        if value.contains("//") || value.contains("\\\\") {
            return Err(ToolValidationError::SecurityViolation(
                "Suspicious path pattern detected".to_string(),
            ));
        }

        Ok(())
    }

    /// Check for command injection attempts
    pub(super) fn check_command_injection(&self, value: &str) -> Result<(), ToolValidationError> {
        let dangerous_chars = [';', '&', '|', '$', '`', '\n', '\r'];

        for ch in &dangerous_chars {
            if value.contains(*ch) {
                return Err(ToolValidationError::SecurityViolation(format!(
                    "Command injection attempt detected: contains '{}'",
                    ch
                )));
            }
        }

        // Check for command substitution patterns
        if value.contains("$(") || value.contains("${") {
            return Err(ToolValidationError::SecurityViolation(
                "Command substitution pattern detected".to_string(),
            ));
        }

        Ok(())
    }
}

impl ToolValidator for SecurityValidator {
    fn validate(&self, tool: &dyn Tool) -> Result<(), ToolValidationError> {
        // Check tool description for potential issues
        let desc = tool.description();
        if desc.len() < 10 {
            return Err(ToolValidationError::InvalidSchema(
                "Tool description must be at least 10 characters".to_string(),
            ));
        }

        Ok(())
    }

    fn validate_input(&self, _tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        // Recursively check all string values for security issues
        fn check_value(
            value: &Value,
            validator: &SecurityValidator,
        ) -> Result<(), ToolValidationError> {
            match value {
                Value::String(s) => {
                    validator.check_path_traversal(s)?;
                    validator.check_command_injection(s)?;
                    Ok(())
                }
                Value::Array(arr) => {
                    for item in arr {
                        check_value(item, validator)?;
                    }
                    Ok(())
                }
                Value::Object(obj) => {
                    for (k, v) in obj {
                        // Also check keys for path traversal in property names
                        validator.check_path_traversal(k)?;
                        check_value(v, validator)?;
                    }
                    Ok(())
                }
                _ => Ok(()),
            }
        }

        check_value(args, self)
    }
}
