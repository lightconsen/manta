//! macOS Accessibility tool — query UI trees via AppleScript/System Events.

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tracing::{info, warn};

use super::applescript::AppleScriptTool;
use crate::tools::{create_schema, Tool, ToolContext, ToolExecutionResult};

/// Description of a UI element on macOS.
#[derive(Debug, Clone, Serialize)]
pub struct UiElement {
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub enabled: Option<bool>,
    pub position: Option<String>,
    pub size: Option<String>,
    pub children: Vec<UiElement>,
}

/// Result of an accessibility query.
#[derive(Debug, Clone, Serialize)]
pub struct AccessibilityResult {
    pub success: bool,
    pub app: Option<String>,
    pub elements: Vec<UiElement>,
    pub raw_output: Option<String>,
    pub error: Option<String>,
}

/// Query the macOS accessibility tree using AppleScript/System Events.
///
/// This is the first step of the hybrid desktop perception model:
/// structured UI tree first, screenshot only when needed.
#[derive(Debug)]
pub struct AccessibilityTool;

impl Default for AccessibilityTool {
    fn default() -> Self {
        Self::new()
    }
}

impl AccessibilityTool {
    pub fn new() -> Self {
        Self
    }

    /// Build an AppleScript that enumerates UI elements of a target
    /// application.
    fn build_ui_tree_script(app_name: &str) -> String {
        format!(
            r#"tell application "System Events"
    tell application process "{}"
        set frontmost to true
        set _output to ""
        set _indent to "  "

        repeat with _win in (get every window)
            set _output to _output & "window|" & (name of _win) & "|enabled|" & (enabled of _win) & "\n"
            set _output to _output & my describe_elements(_win, _indent)
        end repeat

        return _output
    end tell
end tell

on describe_elements(_container, _indent)
    tell application "System Events"
        set _result to ""
        try
            repeat with _elem in (get every UI element of _container)
                try
                    set _role to role of _elem as string
                on error
                    set _role to "unknown"
                end try
                try
                    set _name to name of _elem as string
                on error
                    set _name to ""
                end try
                try
                    set _value to value of _elem as string
                on error
                    set _value to ""
                end try
                try
                    set _enabled to enabled of _elem as string
                on error
                    set _enabled to ""
                end try
                try
                    set _pos to position of _elem as string
                on error
                    set _pos to ""
                end try
                try
                    set _sz to size of _elem as string
                on error
                    set _sz to ""
                end try

                set _result to _result & _indent & _role & "|" & _name & "|" & _value & "|" & _enabled & "|" & _pos & "|" & _sz & "\n"
                set _result to _result & my describe_elements(_elem, _indent & "  ")
            end repeat
        end try
        return _result
    end tell
end describe_elements"#,
            app_name
        )
    }

    fn build_frontmost_script() -> String {
        r#"tell application "System Events"
    set _proc to first application process whose frontmost is true
    set _name to name of _proc
    set _output to "app|" & _name & "\n"

    tell _proc
        repeat with _win in (get every window)
            try
                set _win_name to name of _win
            on error
                set _win_name to ""
            end try
            set _output to _output & "window|" & _win_name & "|enabled|" & (enabled of _win) & "\n"
            set _output to _output & my describe_elements(_win, "  ")
        end repeat
    end tell

    return _output
end tell

on describe_elements(_container, _indent)
    tell application "System Events"
        set _result to ""
        try
            repeat with _elem in (get every UI element of _container)
                try
                    set _role to role of _elem as string
                on error
                    set _role to "unknown"
                end try
                try
                    set _name to name of _elem as string
                on error
                    set _name to ""
                end try
                try
                    set _value to value of _elem as string
                on error
                    set _value to ""
                end try
                try
                    set _enabled to enabled of _elem as string
                on error
                    set _enabled to ""
                end try
                try
                    set _pos to position of _elem as string
                on error
                    set _pos to ""
                end try
                try
                    set _sz to size of _elem as string
                on error
                    set _sz to ""
                end try

                set _result to _result & _indent & _role & "|" & _name & "|" & _value & "|" & _enabled & "|" & _pos & "|" & _sz & "\n"
                set _result to _result & my describe_elements(_elem, _indent & "  ")
            end repeat
        end try
        return _result
    end tell
end describe_elements"#
        .to_string()
    }

    /// Parse the pipe-delimited output from AppleScript into a flat list.
    ///
    /// For LLM consumption a flat list with indentation depth is usually
    /// sufficient and avoids complex tree-building bugs.
    fn parse_tree_output(output: &str) -> Vec<UiElement> {
        let mut elements: Vec<UiElement> = Vec::new();

        for line in output.lines() {
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                continue;
            }

            let parts: Vec<&str> = trimmed.trim_start().split('|').collect();
            if parts.len() < 2 {
                continue;
            }

            let role = parts[0].to_string();
            // The first line is app metadata, not part of the visual tree.
            if role == "app" {
                continue;
            }

            elements.push(UiElement {
                role,
                name: parts[1].to_string(),
                value: parts
                    .get(2)
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
                enabled: parts.get(3).and_then(|s| match *s {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                }),
                position: parts
                    .get(4)
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
                size: parts
                    .get(5)
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty()),
                children: Vec::new(),
            });
        }

        elements
    }
}

#[async_trait]
impl Tool for AccessibilityTool {
    fn name(&self) -> &str {
        "macos_accessibility"
    }

    fn description(&self) -> &str {
        "Query the macOS accessibility/UI tree for a specific application or the frontmost \
         application. Returns a structured tree of windows, buttons, text fields, and other UI \
         elements. This is the primary perception mechanism for desktop control — use it before \
         taking screenshots whenever possible."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Query UI tree via Accessibility API",
            serde_json::json!({
                "app": {
                    "type": "string",
                    "description": "Application name to inspect (e.g. Safari, Finder). If omitted, inspects the frontmost application."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum recursion depth (AppleScript may time out for deep trees)",
                    "default": 3
                }
            }),
            Vec::<String>::new(),
        )
    }

    async fn execute(
        &self,
        args: Value,
        _context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        // Early-exit with a clear error if accessibility permission is missing.
        if !super::permissions::has_accessibility_permission() {
            let guide = super::permissions::accessibility_permission_guide();
            let err_msg = format!(
                "macOS Accessibility permission not granted. Please grant it in System Settings → \
                 Privacy & Security → Accessibility.\n\n{}",
                guide
            );
            warn!("Accessibility tool blocked: {}", err_msg);
            return Ok(ToolExecutionResult::error(err_msg.clone()).with_data(serde_json::json!({
                "success": false,
                "error": err_msg,
                "needs_permission": true,
            })));
        }

        let script = if let Some(app) = args["app"].as_str() {
            info!("Querying accessibility tree for app: {}", app);
            Self::build_ui_tree_script(app)
        } else {
            info!("Querying accessibility tree for frontmost app");
            Self::build_frontmost_script()
        };

        let as_result =
            AppleScriptTool::execute_script(&script, args["timeout"].as_u64().unwrap_or(15)).await;

        let mut result = AccessibilityResult {
            success: as_result.success,
            app: None,
            elements: Vec::new(),
            raw_output: Some(as_result.output.clone()),
            error: as_result.error.clone(),
        };

        if as_result.success {
            result.app = as_result
                .output
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("app|").map(|s| s.to_string()));
            result.elements = Self::parse_tree_output(&as_result.output);
        } else {
            warn!(
                "Accessibility query failed: {}",
                as_result.error.as_deref().unwrap_or("unknown error")
            );
        }

        let json = serde_json::to_string_pretty(&result)
            .map_err(crate::error::SyscityError::Serialization)?;

        if result.success {
            Ok(ToolExecutionResult::success(json).with_data(serde_json::to_value(result)?))
        } else {
            Ok(ToolExecutionResult::error(result.error.clone().unwrap_or_default())
                .with_data(serde_json::to_value(result)?))
        }
    }

    fn is_available(&self, _context: &ToolContext) -> bool {
        cfg!(target_os = "macos")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accessibility_tool_creation() {
        let tool = AccessibilityTool::new();
        assert_eq!(tool.name(), "macos_accessibility");
        assert!(tool.description().contains("UI tree"));
    }

    #[test]
    fn test_parse_tree_output() {
        let output = "app|Safari\n\
                      window|GitHub|enabled|true\n\
                        button|Reload||true||\n\
                        text field|Search||false|{100, 200}|{300, 30}\n\
                          static text|https://github.com|https://github.com||";
        let elements = AccessibilityTool::parse_tree_output(output);
        // app line is skipped, remaining 4 elements are flattened
        assert_eq!(elements.len(), 4);
        assert_eq!(elements[0].role, "window");
        assert_eq!(elements[1].role, "button");
        assert_eq!(elements[2].role, "text field");
        assert_eq!(elements[3].role, "static text");
        assert_eq!(elements[2].position.as_deref(), Some("{100, 200}"));
    }
}
