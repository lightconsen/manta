//! Task classification for cost-aware routing
//!
//! Defines the [`TaskClassifierImpl`] trait for pluggable classifiers and a
//! keyword-based implementation ([`KeywordTaskClassifier`]) extracted from the
//! original monolithic `mod.rs`.
//!
//! # Pluggability
//!
//! Users can provide their own classifier (e.g. a lightweight ML model) by
//! implementing [`TaskClassifierImpl`] and attaching it to the router via
//! [`with_classifier`](crate::model_router::ModelRouter::with_classifier).

use crate::model_router::TaskType;
use crate::providers::{Message, Role};

/// Trait for pluggable task classifiers.
///
/// Implementors receive the conversation messages and return a [`TaskType`].
/// The default implementation is [`KeywordTaskClassifier`].
pub trait TaskClassifierImpl: Send + Sync {
    /// Classify a conversation into a task type.
    fn classify(&self, messages: &[Message]) -> TaskType;
}

/// Lightweight rule-based task classifier using keyword scoring.
///
/// Categories are evaluated in priority order and the first whose score meets
/// the threshold wins. Negative guards suppress false positives (e.g.
/// "function" in non-coding contexts).
pub struct KeywordTaskClassifier;

impl TaskClassifierImpl for KeywordTaskClassifier {
    fn classify(&self, messages: &[Message]) -> TaskType {
        let text: String = messages
            .iter()
            .filter(|m| matches!(m.role, Role::User))
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let lower = text.to_lowercase();

        // Coding: strong signals win outright; weaker signals need reinforcement.
        if !contains_any(
            &lower,
            &[
                "no code",
                "not code",
                "without code",
                "don't code",
                "don't write code",
                "don't need code",
                "no need for code",
                "not need code",
                "no coding",
                "not coding",
                "不需要代码",
                "不要代码",
                "不用代码",
                "不需要写代码",
            ],
        ) && score(
            &lower,
            &[
                "code",
                "coding",
                "programming",
                "algorithm",
                "debug",
                "refactor",
                "write a function",
                "write a script",
                "```",
            ],
            &["function", "bug", "implement"],
        ) >= 2
        {
            return TaskType::Coding;
        }

        // Reasoning
        if !contains_any(
            &lower,
            &[
                "no reason",
                "not reason",
                "don't explain",
                "no explanation",
                "不需要理由",
                "不用解释",
                "不要解释",
            ],
        ) && score(
            &lower,
            &[
                "explain why",
                "analyze",
                "compare",
                "evaluate",
                "prove",
                "step by step",
                "why does",
                "how does",
            ],
            &["reason", "logic", "explain"],
        ) >= 2
        {
            return TaskType::Reasoning;
        }

        // Summarization
        if !contains_any(
            &lower,
            &[
                "don't summarize",
                "no summary",
                "不要总结",
                "不需要总结",
                "不用总结",
            ],
        ) && score(&lower, &["summarize", "summary", "tl;dr", "key points", "main ideas"], &[])
            >= 2
        {
            return TaskType::Summarization;
        }

        // Classification
        if !contains_any(
            &lower,
            &[
                "don't classify",
                "no classification",
                "不要分类",
                "不需要分类",
            ],
        ) && score(
            &lower,
            &["classify", "categor", "label", "sentiment", "what type"],
            &["is this"],
        ) >= 2
        {
            return TaskType::Classification;
        }

        // Translation
        if !contains_any(
            &lower,
            &[
                "don't translate",
                "no translation",
                "不要翻译",
                "不需要翻译",
            ],
        ) && score(&lower, &["translate", "translation"], &[]) >= 2
        {
            return TaskType::Translation;
        }

        // Extraction
        if !contains_any(&lower, &["don't extract", "no extraction", "不要提取", "不需要提取"])
            && score(
                &lower,
                &["extract", "pull out", "find all", "list the"],
                &["parse", "get the"],
            ) >= 2
        {
            return TaskType::Extraction;
        }

        // Creative
        if !contains_any(
            &lower,
            &[
                "don't write",
                "no story",
                "no poem",
                "不要写",
                "不要故事",
                "不要诗歌",
            ],
        ) && score(
            &lower,
            &[
                "write a story",
                "poem",
                "poetry",
                "creative writing",
                "compose a",
            ],
            &["draft", "rewrite", "creative", "compose"],
        ) >= 2
        {
            return TaskType::Creative;
        }

        TaskType::Chat
    }
}

/// True if `text` contains any of the `phrases`.
fn contains_any(text: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|p| text.contains(p))
}

/// Score `text` against strong and weak keyword lists.
///
/// Strong matches count for 2 points, weak matches for 1 point.
fn score(text: &str, strong: &[&str], weak: &[&str]) -> u32 {
    let strong_hits = strong.iter().filter(|p| text.contains(**p)).count() as u32;
    let weak_hits = weak.iter().filter(|p| text.contains(**p)).count() as u32;
    strong_hits * 2 + weak_hits
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_classifier_detects_coding() {
        let msgs = vec![Message::user("Write a function to sort an array in Python")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Coding);
    }

    #[test]
    fn task_classifier_detects_summarization() {
        let msgs = vec![Message::user("Summarize this article for me")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Summarization);
    }

    #[test]
    fn task_classifier_detects_reasoning() {
        let msgs = vec![Message::user("Explain why the sky is blue step by step")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Reasoning);
    }

    #[test]
    fn task_classifier_defaults_to_chat() {
        let msgs = vec![Message::user("Hello, how are you today?")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Chat);
    }

    #[test]
    fn task_classifier_detects_classification() {
        let msgs = vec![Message::user("Classify this text as positive or negative")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Classification);
    }

    #[test]
    fn task_classifier_detects_translation() {
        let msgs = vec![Message::user("Translate this to French")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Translation);
    }

    #[test]
    fn task_classifier_detects_extraction() {
        let msgs = vec![Message::user("Extract all email addresses from this text")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Extraction);
    }

    #[test]
    fn task_classifier_negative_guard_avoids_coding() {
        let msgs = vec![Message::user(
            "I don't need code, just explain why this happens",
        )];
        let classifier = KeywordTaskClassifier;
        assert_ne!(classifier.classify(&msgs), TaskType::Coding);
    }

    #[test]
    fn task_classifier_weak_function_is_not_coding() {
        let msgs = vec![Message::user("The function of this device is unclear")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Chat);
    }

    #[test]
    fn task_classifier_is_this_stays_chat() {
        let msgs = vec![Message::user("Is this a good idea?")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Chat);
    }

    #[test]
    fn task_classifier_dont_summarize_stays_chat() {
        let msgs = vec![Message::user("Don't summarize, just chat with me")];
        let classifier = KeywordTaskClassifier;
        assert_eq!(classifier.classify(&msgs), TaskType::Chat);
    }

    #[test]
    fn task_classifier_chinese_negative_guard() {
        let msgs = vec![Message::user("不需要代码，只要解释原因")];
        let classifier = KeywordTaskClassifier;
        assert_ne!(classifier.classify(&msgs), TaskType::Coding);
    }
}
