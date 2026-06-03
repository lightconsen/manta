use serde::Serialize;

/// Heartbeat status for a single run
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum HeartbeatStatus {
    /// Heartbeat completed normally, no tasks needed attention
    Ok,
    /// A task from HEARTBEAT.md was executed
    TaskExecuted,
    /// Agent was idle, no response needed
    Idle,
    /// No agents are registered for heartbeat
    NoAgents,
}

/// Events emitted by the heartbeat runner
#[derive(Debug, Clone)]
pub enum HeartbeatEvent {
    /// Heartbeat cycle started
    Started,
    /// Heartbeat completed for an agent
    Completed {
        status: HeartbeatStatus,
        agent_id: String,
        session_id: Option<String>,
    },
    /// Heartbeat skipped for an agent
    Skipped { reason: String, agent_id: String },
    /// Heartbeat failed for an agent
    Failed { error: String, agent_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heartbeat_status_copy() {
        let status = HeartbeatStatus::Idle;
        let _copy = status;
        let _ = status; // Should compile because Copy is derived
    }

    #[test]
    fn test_heartbeat_status_serde() {
        // Verify variants are distinct
        assert_ne!(HeartbeatStatus::Ok, HeartbeatStatus::Idle);
        assert_ne!(HeartbeatStatus::TaskExecuted, HeartbeatStatus::Idle);
        assert_ne!(HeartbeatStatus::NoAgents, HeartbeatStatus::Ok);
    }

    #[test]
    fn test_heartbeat_event_started() {
        let event = HeartbeatEvent::Started;
        match event {
            HeartbeatEvent::Started => {}
            _ => panic!("Expected Started event"),
        }
    }

    #[test]
    fn test_heartbeat_event_completed() {
        let event = HeartbeatEvent::Completed {
            status: HeartbeatStatus::Idle,
            agent_id: "default".to_string(),
            session_id: Some("heartbeat:default".to_string()),
        };
        match event {
            HeartbeatEvent::Completed { status, agent_id, session_id } => {
                assert_eq!(status, HeartbeatStatus::Idle);
                assert_eq!(agent_id, "default");
                assert_eq!(session_id, Some("heartbeat:default".to_string()));
            }
            _ => panic!("Expected Completed event"),
        }
    }

    #[test]
    fn test_heartbeat_event_skipped() {
        let event = HeartbeatEvent::Skipped {
            reason: "agent_busy".to_string(),
            agent_id: "default".to_string(),
        };
        match event {
            HeartbeatEvent::Skipped { reason, agent_id } => {
                assert_eq!(reason, "agent_busy");
                assert_eq!(agent_id, "default");
            }
            _ => panic!("Expected Skipped event"),
        }
    }

    #[test]
    fn test_heartbeat_event_failed() {
        let event = HeartbeatEvent::Failed {
            error: "timeout".to_string(),
            agent_id: "default".to_string(),
        };
        match event {
            HeartbeatEvent::Failed { error, agent_id } => {
                assert_eq!(error, "timeout");
                assert_eq!(agent_id, "default");
            }
            _ => panic!("Expected Failed event"),
        }
    }
}
