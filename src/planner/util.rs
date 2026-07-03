//! Shared utility functions for the planner module.

/// Strip markdown code fences (` ```json … ``` ` or ` ``` … ``` `) from a
/// string, returning the content inside the fences.  If no fences are found
/// the input is returned as-is.
pub fn strip_code_fences(text: &str) -> &str {
    let trimmed = text.trim();
    if trimmed.starts_with("```json") {
        trimmed
            .strip_prefix("```json")
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim()
    } else if trimmed.starts_with("```") {
        trimmed
            .strip_prefix("```")
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim()
    } else {
        trimmed
    }
}
