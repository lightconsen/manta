//! Content filter — scan tool outputs for PII and secrets before returning to
//! users.
//!
//! Combines [`PiiDetector`] and [`SecretScanner`] to automatically redact or
//! block sensitive data in [`ToolExecutionResult`]s.
//!
//! # Usage
//!
//! ```rust
//! use syscity::security::content_filter::ContentFilter;
//! use syscity::tools::ToolExecutionResult;
//!
//! let filter = ContentFilter::default();
//! let result = ToolExecutionResult::success("Email: alice@example.com");
//! let outcome = filter.filter_result(&result);
//! assert!(outcome.output.contains("al***@example.com"));
//! ```

use serde_json::Value;

use crate::security::pii::{DataClassification, DetectedPii, FilterResult, PiiDetector};
use crate::security::secrets::{DetectedSecret, SecretScanner};
use crate::tools::ToolExecutionResult;

/// Action taken by the content filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    /// No sensitive content found — passed through unchanged.
    Pass,
    /// Sensitive content was redacted.
    Redacted,
    /// Response contained Restricted-classified PII and was blocked.
    Blocked,
}

/// Outcome of filtering a [`ToolExecutionResult`].
#[derive(Debug, Clone)]
pub struct ContentFilterOutcome {
    /// The (possibly redacted) output string.
    pub output: String,
    /// The (possibly redacted) structured data.
    pub data: Option<Value>,
    /// Whether the original result was successful.
    pub success: bool,
    /// Action taken by the filter.
    pub action: FilterAction,
    /// PII findings (empty if none).
    pub pii_findings: Vec<DetectedPii>,
    /// Secret findings (empty if none).
    pub secret_findings: Vec<DetectedSecret>,
    /// Human-readable summary of actions taken.
    pub summary: String,
}

/// Content filter combining PII detection and secret scanning.
///
/// Configurable: you can enable/disable blocking, redaction, and secret
/// scanning independently.
#[derive(Debug, Clone)]
pub struct ContentFilter {
    pii_detector: PiiDetector,
    secret_scanner: SecretScanner,
    block_restricted: bool,
    redact_confidential: bool,
    redact_secrets: bool,
}

impl Default for ContentFilter {
    fn default() -> Self {
        Self {
            pii_detector: PiiDetector::with_default_patterns(),
            secret_scanner: SecretScanner::with_default_patterns(),
            block_restricted: true,
            redact_confidential: true,
            redact_secrets: true,
        }
    }
}

impl ContentFilter {
    /// Create a new content filter with default patterns.
    pub fn new() -> Self {
        Self::default()
    }

    /// Disable blocking of Restricted-classified PII (default: enabled).
    pub fn with_block_restricted(mut self, enabled: bool) -> Self {
        self.block_restricted = enabled;
        self
    }

    /// Disable redaction of Confidential-classified PII (default: enabled).
    pub fn with_redact_confidential(mut self, enabled: bool) -> Self {
        self.redact_confidential = enabled;
        self
    }

    /// Disable redaction of secrets (default: enabled).
    pub fn with_redact_secrets(mut self, enabled: bool) -> Self {
        self.redact_secrets = enabled;
        self
    }

    /// Filter a [`ToolExecutionResult`], scanning output and data for PII
    /// and secrets.
    ///
    /// # Logic
    ///
    /// 1. Scan `output` + serialized `data` for PII and secrets.
    /// 2. If any **Restricted** PII is found and `block_restricted` is true →
    ///    return [`FilterAction::Blocked`] with a warning message.
    /// 3. If any **Confidential** PII or secrets are found and redaction is
    ///    enabled → redact them in-place and return [`FilterAction::Redacted`].
    /// 4. Otherwise → return [`FilterAction::Pass`] with original content.
    pub fn filter_result(&self, result: &ToolExecutionResult) -> ContentFilterOutcome {
        let mut combined = result.output.clone();
        let data_str = result
            .data
            .as_ref()
            .map(|d| d.to_string())
            .unwrap_or_default();
        if !data_str.is_empty() {
            combined.push(' ');
            combined.push_str(&data_str);
        }

        // ── PII scan ──────────────────────────────────────────────────────
        let pii_result = self.pii_detector.filter_response(&combined);

        let mut pii_findings: Vec<DetectedPii> = Vec::new();
        let mut secret_findings: Vec<DetectedSecret> = Vec::new();
        let mut action = FilterAction::Pass;
        let mut output = result.output.clone();
        let mut data = result.data.clone();

        self.apply_pii_result(
            &pii_result,
            result,
            &mut pii_findings,
            &mut action,
            &mut output,
            &mut data,
        );

        // ── Secret scan ───────────────────────────────────────────────────
        if action != FilterAction::Blocked && self.redact_secrets {
            self.apply_secret_redaction(
                result,
                &mut action,
                &mut output,
                &mut data,
                &mut secret_findings,
            );
        }

        let summary = self.build_summary(&action, &pii_findings, &secret_findings);

        ContentFilterOutcome {
            output,
            data,
            success: result.success,
            action,
            pii_findings,
            secret_findings,
            summary,
        }
    }

    /// Quick check: does this result contain any sensitive content?
    pub fn contains_sensitive(&self, result: &ToolExecutionResult) -> bool {
        let mut combined = result.output.clone();
        if let Some(ref d) = result.data {
            combined.push(' ');
            combined.push_str(&d.to_string());
        }
        self.pii_detector.contains_pii(&combined) || !self.secret_scanner.scan(&combined).is_empty()
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    /// Process the PII scan result: decide to pass, redact, or block.
    fn apply_pii_result(
        &self,
        pii_result: &FilterResult,
        result: &ToolExecutionResult,
        pii_findings: &mut Vec<DetectedPii>,
        action: &mut FilterAction,
        output: &mut String,
        data: &mut Option<Value>,
    ) {
        match pii_result {
            FilterResult::Clean(_) => {}
            FilterResult::Redacted(_redacted_text, findings) => {
                *pii_findings = findings.clone();
                if self.redact_confidential {
                    *action = FilterAction::Redacted;
                    let (new_output, _) = self.pii_detector.redact_text(&result.output);
                    *output = new_output;
                    if let Some(ref d) = result.data {
                        let (redacted_data_text, _) = self.pii_detector.redact_text(&d.to_string());
                        *data = serde_json::from_str(&redacted_data_text)
                            .ok()
                            .or(Some(Value::String(redacted_data_text)));
                    }
                }
            }
            FilterResult::Blocked(findings) => {
                *pii_findings = findings.clone();
                if self.block_restricted {
                    *action = FilterAction::Blocked;
                    *output = "This response contains sensitive personal information and has been \
                               blocked. Please review the content before sharing."
                        .to_string();
                    *data = None;
                } else if self.redact_confidential {
                    *action = FilterAction::Redacted;
                    let (new_output, _) = self.pii_detector.redact_text(&result.output);
                    *output = new_output;
                    if let Some(ref d) = result.data {
                        let (redacted_data_text, _) = self.pii_detector.redact_text(&d.to_string());
                        *data = serde_json::from_str(&redacted_data_text)
                            .ok()
                            .or(Some(Value::String(redacted_data_text)));
                    }
                }
            }
        }
    }

    /// Re-scan output and data for secrets, replace each original match with
    /// its redacted form.
    fn apply_secret_redaction(
        &self,
        result: &ToolExecutionResult,
        action: &mut FilterAction,
        output: &mut String,
        data: &mut Option<Value>,
        secret_findings: &mut Vec<DetectedSecret>,
    ) {
        let secret_output_findings = self.secret_scanner.scan(&result.output);
        let secret_data_findings = result
            .data
            .as_ref()
            .map(|d| self.secret_scanner.scan(&d.to_string()))
            .unwrap_or_default();

        let all_secret_findings: Vec<_> = secret_output_findings
            .into_iter()
            .chain(secret_data_findings)
            .collect();

        if all_secret_findings.is_empty() {
            return;
        }

        for finding in &all_secret_findings {
            *output = output.replace(&finding.original, &finding.redacted);
            if let Some(ref d) = result.data {
                let data_text = d.to_string();
                let new_data_text = data_text.replace(&finding.original, &finding.redacted);
                *data = serde_json::from_str(&new_data_text)
                    .ok()
                    .or(Some(Value::String(new_data_text)));
            }
        }
        if *action == FilterAction::Pass {
            *action = FilterAction::Redacted;
        }
        *secret_findings = self.dedup_secrets(all_secret_findings);
    }

    fn dedup_secrets(&self, secrets: Vec<DetectedSecret>) -> Vec<DetectedSecret> {
        let mut seen = std::collections::HashSet::new();
        secrets
            .into_iter()
            .filter(|s| {
                let key = format!("{}:{}:{}", s.pattern, s.line_number, s.redacted);
                seen.insert(key)
            })
            .collect()
    }

    fn build_summary(
        &self,
        action: &FilterAction,
        pii: &[DetectedPii],
        secrets: &[DetectedSecret],
    ) -> String {
        match action {
            FilterAction::Pass => "No sensitive content detected".to_string(),
            FilterAction::Blocked => format!(
                "Blocked: {} restricted PII item(s) detected",
                pii.iter()
                    .filter(|f| f.classification == DataClassification::Restricted)
                    .count()
            ),
            FilterAction::Redacted => {
                let mut parts = Vec::new();
                let confidential_count = pii
                    .iter()
                    .filter(|f| f.classification == DataClassification::Confidential)
                    .count();
                let internal_count = pii
                    .iter()
                    .filter(|f| f.classification == DataClassification::Internal)
                    .count();
                if confidential_count > 0 {
                    parts.push(format!("{} confidential PII redacted", confidential_count));
                }
                if internal_count > 0 {
                    parts.push(format!("{} internal PII logged", internal_count));
                }
                if !secrets.is_empty() {
                    parts.push(format!("{} secret(s) redacted", secrets.len()));
                }
                if parts.is_empty() {
                    "Content redacted".to_string()
                } else {
                    parts.join("; ")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(output: impl Into<String>) -> ToolExecutionResult {
        ToolExecutionResult::success(output)
    }

    #[test]
    fn test_clean_content_passes_through() {
        let filter = ContentFilter::default();
        let result = make_result("Hello world, no sensitive data here.");
        let outcome = filter.filter_result(&result);
        assert_eq!(outcome.action, FilterAction::Pass);
        assert_eq!(outcome.output, "Hello world, no sensitive data here.");
    }

    #[test]
    fn test_email_redacted() {
        let filter = ContentFilter::default();
        let result = make_result("Contact me at alice@example.com");
        let outcome = filter.filter_result(&result);
        assert_eq!(outcome.action, FilterAction::Redacted);
        assert!(outcome.output.contains("al***@example.com"));
        assert!(!outcome.output.contains("alice@example.com"));
    }

    #[test]
    fn test_chinese_id_blocked() {
        let filter = ContentFilter::default();
        // Valid Chinese ID: 11010119900101127X
        let result = make_result("My ID is 11010119900101127X");
        let outcome = filter.filter_result(&result);
        assert_eq!(outcome.action, FilterAction::Blocked);
        assert!(outcome.output.contains("blocked"));
    }

    #[test]
    fn test_bank_card_redacted() {
        let filter = ContentFilter::default();
        // Valid Visa test number (passes Luhn)
        let result = make_result("Card: 4111111111111111");
        let outcome = filter.filter_result(&result);
        assert_eq!(outcome.action, FilterAction::Redacted);
        assert!(outcome.output.contains("4111****1111"));
    }

    #[test]
    fn test_secret_api_key_redacted() {
        let filter = ContentFilter::default();
        let result = make_result("Key: sk-abcdefghijklmnopqrstuvwxyz123456789012345678901234567");
        let outcome = filter.filter_result(&result);
        assert_eq!(outcome.action, FilterAction::Redacted);
        assert!(!outcome.output.contains("sk-abcdefghijkl"));
    }

    #[test]
    fn test_data_field_redacted() {
        let filter = ContentFilter::default();
        let mut result = make_result("User data");
        result.data = Some(serde_json::json!({
            "email": "alice@example.com",
            "name": "Alice"
        }));
        let outcome = filter.filter_result(&result);
        assert_eq!(outcome.action, FilterAction::Redacted);
        let data = outcome.data.unwrap();
        let email = data["email"].as_str().unwrap();
        assert!(email.contains("al***@example.com"), "Expected redacted email, got: {}", email);
    }

    #[test]
    fn test_no_block_when_disabled() {
        let filter = ContentFilter::new().with_block_restricted(false);
        let result = make_result("ID: 11010119900101127X");
        let outcome = filter.filter_result(&result);
        // Should redact instead of block
        assert_eq!(outcome.action, FilterAction::Redacted);
        assert!(outcome.output.contains("110101********127X"));
    }

    #[test]
    fn test_contains_sensitive_detects_pii() {
        let filter = ContentFilter::default();
        let result = make_result("Call me at 13800138000");
        assert!(filter.contains_sensitive(&result));
    }

    #[test]
    fn test_contains_sensitive_false_for_clean() {
        let filter = ContentFilter::default();
        let result = make_result("Just some regular text");
        assert!(!filter.contains_sensitive(&result));
    }
}
