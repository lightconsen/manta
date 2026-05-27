/// Priority for heartbeat wake requests
///
/// Mirrors OpenClaw's priority system:
/// retry < interval < default < action
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum WakePriority {
    /// Lowest priority — retry for busy agents
    Retry,
    /// Normal interval-based heartbeat
    Interval,
    /// Default manual wake
    Default,
    /// Action-triggered wake (from cron, user request, etc.)
    Action,
}

/// Request to wake up an agent for a heartbeat check
#[derive(Debug, Clone)]
pub struct WakeRequest {
    pub agent_id: String,
    pub priority: WakePriority,
    pub prompt: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wake_priority_ordering() {
        // Retry < Interval < Default < Action
        assert!(WakePriority::Retry < WakePriority::Interval);
        assert!(WakePriority::Interval < WakePriority::Default);
        assert!(WakePriority::Default < WakePriority::Action);
        assert!(WakePriority::Retry < WakePriority::Action);
    }

    #[test]
    fn test_wake_priority_equality() {
        assert_eq!(WakePriority::Retry, WakePriority::Retry);
        assert_eq!(WakePriority::Action, WakePriority::Action);
        assert_ne!(WakePriority::Retry, WakePriority::Action);
    }

    #[test]
    fn test_wake_request_creation() {
        let req = WakeRequest {
            agent_id: "default".to_string(),
            priority: WakePriority::Action,
            prompt: Some("Check logs".to_string()),
        };
        assert_eq!(req.agent_id, "default");
        assert_eq!(req.priority, WakePriority::Action);
        assert_eq!(req.prompt.as_deref(), Some("Check logs"));
    }

    #[test]
    fn test_wake_request_without_prompt() {
        let req = WakeRequest {
            agent_id: "default".to_string(),
            priority: WakePriority::Interval,
            prompt: None,
        };
        assert!(req.prompt.is_none());
    }
}
