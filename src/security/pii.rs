//! PII (Personally Identifiable Information) detection and content filtering.
//!
//! Detects Chinese ID numbers, bank card numbers, phone numbers, and email
//! addresses in text. Supports redaction (masking), confidence scoring, and
//! data classification tiers.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Data classification tiers for detected content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DataClassification {
    /// No restriction — freely distributable.
    Public = 0,
    /// Logged but allowed through.
    Internal = 1,
    /// Auto-redacted in output.
    Confidential = 2,
    /// Blocked — requires human approval.
    Restricted = 3,
}

impl std::fmt::Display for DataClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataClassification::Public => write!(f, "public"),
            DataClassification::Internal => write!(f, "internal"),
            DataClassification::Confidential => write!(f, "confidential"),
            DataClassification::Restricted => write!(f, "restricted"),
        }
    }
}

/// A single PII detection pattern.
#[derive(Debug, Clone)]
pub struct PiiPattern {
    /// Pattern name
    pub name: &'static str,
    /// Regex for detection
    pub regex: Regex,
    /// Classification tier
    pub classification: DataClassification,
    /// Base confidence (0.0–1.0)
    pub confidence: f32,
    /// Human-readable description
    pub description: &'static str,
    /// Whether to run extra validation (Luhn, checksum, etc.)
    pub validate: fn(&str) -> bool,
    /// Redaction function: given the raw match, return masked text
    pub redact: fn(&str) -> String,
}

/// A detected PII item.
#[derive(Debug, Clone)]
pub struct DetectedPii {
    /// Pattern name (e.g. "chinese_id", "bank_card")
    pub pattern: String,
    /// Classification tier
    pub classification: DataClassification,
    /// Line number where found (1-indexed)
    pub line_number: usize,
    /// Redacted / masked text for safe display
    pub redacted: String,
    /// Confidence score (0.0–1.0)
    pub confidence: f32,
    /// Human-readable description
    pub description: String,
    /// The original matched text (for audit logging — do NOT expose to users)
    pub original: String,
}

/// Result of filtering a response through the PII detector.
#[derive(Debug, Clone)]
pub enum FilterResult {
    /// No PII found — text passes through unchanged.
    Clean(String),
    /// PII found and redacted. Contains the scrubbed text and findings.
    Redacted(String, Vec<DetectedPii>),
    /// Restricted-classified PII found — output should be blocked.
    Blocked(Vec<DetectedPii>),
}

/// PII detector with built-in patterns for Chinese PII.
#[derive(Debug, Clone)]
pub struct PiiDetector {
    patterns: Vec<PiiPattern>,
}

impl Default for PiiDetector {
    fn default() -> Self {
        Self::with_default_patterns()
    }
}

impl PiiDetector {
    /// Create a detector with the built-in PII patterns.
    pub fn with_default_patterns() -> Self {
        let patterns = vec![
            // ── Chinese National ID ──────────────────────────────────────────
            PiiPattern {
                name: "chinese_id",
                regex: Regex::new(r"[1-9]\d{5}(?:18\d{2}|19\d{2}|20\d{2})\d{2}\d{2}\d{3}[\dXx]").unwrap(),
                classification: DataClassification::Restricted,
                confidence: 0.90,
                description: "Chinese national ID number detected",
                validate: validate_chinese_id,
                redact: redact_chinese_id,
            },
            // ── Bank Card (generic, validated with Luhn) ─────────────────────
            PiiPattern {
                name: "bank_card",
                // Matches 13–19 digit sequences that look like card numbers.
                regex: Regex::new(r"\b(?:4[0-9]{12}(?:[0-9]{3})?|5[1-5][0-9]{14}|6(?:011|5[0-9]{2})[0-9]{12}|3[47][0-9]{13}|3(?:0[0-5]|[68][0-9])[0-9]{11}|(?:2131|1800|35\d{3})\d{11}|\d{13,19})\b").unwrap(),
                classification: DataClassification::Confidential,
                confidence: 0.85,
                description: "Bank card number detected",
                validate: validate_luhn,
                redact: redact_bank_card,
            },
            // ── Chinese Mobile Phone ─────────────────────────────────────────
            PiiPattern {
                name: "phone_cn",
                regex: Regex::new(r"(?:\+?86\s?)?1[3-9]\d{9}").unwrap(),
                classification: DataClassification::Internal,
                confidence: 0.80,
                description: "Chinese mobile phone number detected",
                validate: |_s| true,
                redact: redact_phone,
            },
            // ── Landline (with optional area code) ───────────────────────────
            PiiPattern {
                name: "phone_landline",
                regex: Regex::new(r"0\d{2,3}-?\d{7,8}").unwrap(),
                classification: DataClassification::Internal,
                confidence: 0.70,
                description: "Chinese landline phone number detected",
                validate: |_s| true,
                redact: redact_landline,
            },
            // ── Email Address ────────────────────────────────────────────────
            PiiPattern {
                name: "email",
                regex: Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").unwrap(),
                classification: DataClassification::Internal,
                confidence: 0.90,
                description: "Email address detected",
                validate: |_s| true,
                redact: redact_email,
            },
        ];

        Self { patterns }
    }

    /// Create an empty detector (add patterns manually).
    pub fn empty() -> Self {
        Self { patterns: vec![] }
    }

    /// Add a custom pattern.
    pub fn add_pattern(&mut self, pattern: PiiPattern) {
        self.patterns.push(pattern);
    }

    /// Scan text for PII.
    pub fn scan(&self, text: &str) -> Vec<DetectedPii> {
        let mut all = Vec::new();
        for (line_num, line) in text.lines().enumerate() {
            let mut findings = self.scan_line_raw(line, line_num + 1);
            all.append(&mut findings);
        }
        all
    }

    /// Scan a single line.
    pub fn scan_line(&self, line: &str, line_number: usize) -> Vec<DetectedPii> {
        self.scan_line_raw(line, line_number)
    }

    fn scan_line_raw(&self, line: &str, line_number: usize) -> Vec<DetectedPii> {
        let mut raw: Vec<(&PiiPattern, &str, usize, usize, f32)> = Vec::new();

        for pattern in &self.patterns {
            for mat in pattern.regex.find_iter(line) {
                let raw_str = mat.as_str();

                // Boundary check for phone patterns: don't match when
                // surrounded by more digits (avoids matching substrings
                // inside Chinese IDs / bank cards).
                if pattern.name == "phone_cn" || pattern.name == "phone_landline" {
                    let before = mat
                        .start()
                        .checked_sub(1)
                        .and_then(|i| line.as_bytes().get(i));
                    let after = line.as_bytes().get(mat.end());
                    let preceded_by_digit = before.map(|b| b.is_ascii_digit()).unwrap_or(false);
                    let followed_by_digit = after.map(|b| b.is_ascii_digit()).unwrap_or(false);
                    if preceded_by_digit || followed_by_digit {
                        continue;
                    }
                }

                if !(pattern.validate)(raw_str) {
                    continue;
                }

                raw.push((pattern, raw_str, mat.start(), mat.end(), pattern.confidence));
            }
        }

        // Deduplicate overlapping matches, preferring longer / higher-classification.
        raw.sort_by(|a, b| {
            let len_cmp = (b.3 - b.2).cmp(&(a.3 - a.2));
            if len_cmp != std::cmp::Ordering::Equal {
                return len_cmp;
            }
            b.0.classification.cmp(&a.0.classification)
        });

        let mut kept_ranges: Vec<(usize, usize)> = Vec::new();
        let mut findings = Vec::new();

        for (pattern, raw_str, start, end, confidence) in raw {
            let overlaps = kept_ranges.iter().any(|(ks, ke)| start < *ke && end > *ks);
            if overlaps {
                continue;
            }
            kept_ranges.push((start, end));
            findings.push(DetectedPii {
                pattern: pattern.name.to_string(),
                classification: pattern.classification,
                line_number,
                redacted: (pattern.redact)(raw_str),
                confidence,
                description: pattern.description.to_string(),
                original: raw_str.to_string(),
            });
        }

        findings
    }

    /// Check if text contains any PII.
    pub fn contains_pii(&self, text: &str) -> bool {
        !self.scan(text).is_empty()
    }

    /// Redact all PII in text, returning the scrubbed string and findings.
    pub fn redact_text(&self, text: &str) -> (String, Vec<DetectedPii>) {
        let findings = self.scan(text);
        if findings.is_empty() {
            return (text.to_string(), findings);
        }

        let mut result = text.to_string();
        // Replace longest matches first to avoid partial replacements.
        let mut sorted = findings.clone();
        sorted.sort_by(|a, b| b.original.len().cmp(&a.original.len()));

        for finding in &sorted {
            result = result.replace(&finding.original, &finding.redacted);
        }

        (result, findings)
    }

    /// Filter a response: classify findings and decide Clean / Redacted / Blocked.
    pub fn filter_response(&self, text: &str) -> FilterResult {
        let findings = self.scan(text);
        if findings.is_empty() {
            return FilterResult::Clean(text.to_string());
        }

        // If any finding is Restricted, block the whole response.
        let has_restricted = findings
            .iter()
            .any(|f| f.classification == DataClassification::Restricted);
        if has_restricted {
            return FilterResult::Blocked(findings);
        }

        // Otherwise redact.
        let (redacted, _) = self.redact_text(text);
        FilterResult::Redacted(redacted, findings)
    }
}

// ── Validation Functions ────────────────────────────────────────────────────

/// Validate a Chinese national ID number using the GB 11643-1999 checksum.
fn validate_chinese_id(id: &str) -> bool {
    if id.len() != 18 {
        return false;
    }

    // First 17 must be digits
    let prefix = &id[..17];
    if !prefix.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    // Checksum weights
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK_CHARS: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];

    let mut sum = 0u32;
    for (i, ch) in prefix.chars().enumerate() {
        let digit = ch.to_digit(10).unwrap_or(0);
        sum += digit * WEIGHTS[i];
    }

    let check_index = (sum % 11) as usize;
    let expected = CHECK_CHARS[check_index];
    let actual = id.chars().nth(17).unwrap_or(' ');

    actual.to_ascii_uppercase() == expected
}

/// Validate using the Luhn algorithm.
fn validate_luhn(digits: &str) -> bool {
    let clean: Vec<u32> = digits
        .chars()
        .filter(|c| c.is_ascii_digit())
        .filter_map(|c| c.to_digit(10))
        .collect();

    if clean.len() < 13 || clean.len() > 19 {
        return false;
    }

    let mut sum = 0;
    let mut double = false;

    for &digit in clean.iter().rev() {
        let mut value = digit;
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }

    sum % 10 == 0
}

// ── Redaction Functions ─────────────────────────────────────────────────────

/// Redact Chinese ID: keep first 6 + last 4, mask middle with 8 asterisks.
fn redact_chinese_id(id: &str) -> String {
    if id.len() < 10 {
        return "*".repeat(id.len());
    }
    format!("{}********{}", &id[..6], &id[id.len() - 4..])
}

/// Redact bank card: keep first 4 + last 4, mask middle with 4 asterisks.
fn redact_bank_card(card: &str) -> String {
    let digits: String = card.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return "*".repeat(card.len());
    }
    format!("{}****{}", &digits[..4], &digits[digits.len() - 4..])
}

/// Redact mobile phone: keep first 3 + last 4.
fn redact_phone(phone: &str) -> String {
    let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return "*".repeat(phone.len());
    }
    format!("{}****{}", &digits[..3], &digits[digits.len() - 4..])
}

/// Redact landline: keep area code + first 2 of local, mask rest.
fn redact_landline(phone: &str) -> String {
    let digits: String = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return "*".repeat(phone.len());
    }
    // Area code is 2–3 digits, keep them + first 2 of local, mask rest
    let area_len = if digits.starts_with('0') && digits.len() >= 11 {
        3
    } else {
        2
    };
    let keep = area_len + 2;
    if digits.len() > keep + 2 {
        format!("{}****{}", &digits[..keep], &digits[digits.len() - 2..])
    } else {
        format!("{}****", &digits[..keep])
    }
}

/// Redact email: mask local part, keep domain.
fn redact_email(email: &str) -> String {
    if let Some(at_pos) = email.find('@') {
        let local = &email[..at_pos];
        let domain = &email[at_pos..];
        if local.len() <= 2 {
            format!("***{}", domain)
        } else {
            format!("{}***{}", &local[..2], domain)
        }
    } else {
        "***".to_string()
    }
}

// ── Quick Standalone Functions ──────────────────────────────────────────────

/// Quick scan function.
pub fn scan_text(text: &str) -> Vec<DetectedPii> {
    let detector = PiiDetector::with_default_patterns();
    detector.scan(text)
}

/// Quick check function.
pub fn contains_pii(text: &str) -> bool {
    let detector = PiiDetector::with_default_patterns();
    detector.contains_pii(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Chinese ID Tests ────────────────────────────────────────────────────

    #[test]
    fn test_detect_chinese_id_valid() {
        // Valid ID: 11010119900101127X (passes GB 11643-1999 checksum)
        let detector = PiiDetector::with_default_patterns();
        let findings = detector.scan("My ID is 11010119900101127X");
        assert!(!findings.is_empty());
        assert_eq!(findings[0].pattern, "chinese_id");
        assert_eq!(findings[0].classification, DataClassification::Restricted);
        assert!(findings[0].confidence >= 0.9);
    }

    #[test]
    fn test_detect_chinese_id_invalid_checksum() {
        // Same pattern but wrong checksum digit
        let detector = PiiDetector::with_default_patterns();
        let findings = detector.scan("My ID is 110101199001011234");
        // Should NOT be detected because checksum fails
        assert!(findings.is_empty());
    }

    #[test]
    fn test_redact_chinese_id() {
        let redacted = redact_chinese_id("11010119900101127X");
        assert_eq!(redacted, "110101********127X");
    }

    // ── Bank Card Tests ─────────────────────────────────────────────────────

    #[test]
    fn test_detect_bank_card_luhn_valid() {
        // Valid Visa test number: 4111111111111111 (passes Luhn)
        let detector = PiiDetector::with_default_patterns();
        let findings = detector.scan("Card: 4111111111111111");
        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.pattern == "bank_card"));
    }

    #[test]
    fn test_detect_bank_card_luhn_invalid() {
        // 13 random digits that fail Luhn
        let detector = PiiDetector::with_default_patterns();
        let findings = detector.scan("Card: 1234567890123");
        assert!(
            findings.is_empty() || !findings.iter().any(|f| f.pattern == "bank_card"),
            "Should not detect invalid bank card"
        );
    }

    #[test]
    fn test_redact_bank_card() {
        let redacted = redact_bank_card("4111111111111111");
        assert_eq!(redacted, "4111****1111");
    }

    // ── Phone Tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_detect_phone_cn() {
        let detector = PiiDetector::with_default_patterns();
        let findings = detector.scan("Call me at 13800138000");
        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.pattern == "phone_cn"));
    }

    #[test]
    fn test_detect_phone_with_prefix() {
        let detector = PiiDetector::with_default_patterns();
        let findings = detector.scan("Call +86 13800138000 now");
        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.pattern == "phone_cn"));
    }

    #[test]
    fn test_redact_phone() {
        let redacted = redact_phone("13800138000");
        assert_eq!(redacted, "138****8000");
    }

    // ── Email Tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_detect_email() {
        let detector = PiiDetector::with_default_patterns();
        let findings = detector.scan("Contact: alice@example.com");
        assert!(!findings.is_empty());
        assert!(findings.iter().any(|f| f.pattern == "email"));
    }

    #[test]
    fn test_redact_email() {
        let redacted = redact_email("alice@example.com");
        assert_eq!(redacted, "al***@example.com");
    }

    // ── Classification / Filter Tests ───────────────────────────────────────

    #[test]
    fn test_filter_clean() {
        let detector = PiiDetector::with_default_patterns();
        let result = detector.filter_response("Hello world, no PII here.");
        match result {
            FilterResult::Clean(text) => assert_eq!(text, "Hello world, no PII here."),
            _ => panic!("Expected Clean result"),
        }
    }

    #[test]
    fn test_filter_redacted() {
        let detector = PiiDetector::with_default_patterns();
        let result = detector.filter_response("Email me at alice@example.com");
        match result {
            FilterResult::Redacted(text, findings) => {
                assert!(text.contains("al***@example.com"));
                assert!(!findings.is_empty());
                assert_eq!(findings[0].classification, DataClassification::Internal);
            }
            _ => panic!("Expected Redacted result"),
        }
    }

    #[test]
    fn test_filter_blocked_restricted() {
        let detector = PiiDetector::with_default_patterns();
        let result = detector.filter_response("ID: 11010119900101127X");
        match result {
            FilterResult::Blocked(findings) => {
                assert!(!findings.is_empty());
                assert_eq!(findings[0].classification, DataClassification::Restricted);
            }
            _ => panic!("Expected Blocked result for Restricted PII"),
        }
    }

    #[test]
    fn test_redact_text_multiple() {
        let detector = PiiDetector::with_default_patterns();
        let text = "Email: alice@example.com, Phone: 13800138000";
        let (redacted, findings) = detector.redact_text(text);
        assert!(redacted.contains("al***@example.com"));
        assert!(redacted.contains("138****8000"));
        assert_eq!(findings.len(), 2);
    }

    #[test]
    fn test_luhn_validation() {
        assert!(validate_luhn("4111111111111111"));
        assert!(validate_luhn("5555555555554444"));
        assert!(!validate_luhn("1234567890123456"));
        assert!(!validate_luhn("abc"));
    }
}
