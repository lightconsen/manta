use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A single task parsed from HEARTBEAT.md
#[derive(Debug, Clone)]
pub struct HeartbeatTask {
    pub name: String,
    pub interval: Duration,
    pub prompt: String,
}

/// Parse a duration string like "5m", "30s", "1h", "2h30m" into Duration
pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let mut hours: u64 = 0;
    let mut minutes: u64 = 0;
    let mut seconds: u64 = 0;

    let mut current_num = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            current_num.push(c);
        } else if matches!(c, 'h' | 'm' | 's') {
            if let Ok(val) = current_num.parse::<u64>() {
                match c {
                    'h' => hours = val,
                    'm' => minutes = val,
                    's' => seconds = val,
                    _ => {}
                }
            }
            current_num.clear();
        }
    }

    if hours == 0 && minutes == 0 && seconds == 0 {
        return None;
    }

    Some(Duration::from_secs(hours * 3600 + minutes * 60 + seconds))
}

/// Parse heartbeat tasks from HEARTBEAT.md content.
///
/// Supports YAML-like task definitions:
/// ```text
/// tasks:
///   - name: email-check
///     interval: 30m
///     prompt: "Check for urgent unread emails"
/// ```
pub fn parse_heartbeat_tasks(content: &str) -> Vec<HeartbeatTask> {
    let mut tasks: Vec<HeartbeatTask> = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut in_tasks_block = false;
    let mut current_name: Option<String> = None;
    let mut current_interval: Option<String> = None;
    let mut current_prompt: Option<String> = None;

    let flush_task = |name: &mut Option<String>,
                      interval: &mut Option<String>,
                      prompt: &mut Option<String>,
                      tasks: &mut Vec<HeartbeatTask>| {
        if let (Some(n), Some(i), Some(p)) = (name.take(), interval.take(), prompt.take()) {
            if let Some(duration) = parse_duration(&i) {
                tasks.push(HeartbeatTask {
                    name: n,
                    interval: duration,
                    prompt: p.trim().trim_matches('"').trim_matches('\'').to_string(),
                });
            }
        }
    };

    for line in &lines {
        let trimmed = line.trim();

        // Detect tasks block start
        if trimmed == "tasks:" {
            in_tasks_block = true;
            continue;
        }

        // If we hit a new top-level key (non-indented, not empty), exit tasks block
        // Task fields (interval:, prompt:, - name:) should not trigger exit
        let is_task_field = trimmed.starts_with("interval:")
            || trimmed.starts_with("prompt:")
            || trimmed.starts_with("- name:")
            || trimmed.starts_with('-');
        if in_tasks_block
            && !trimmed.is_empty()
            && !trimmed.starts_with(' ')
            && !trimmed.starts_with('\t')
            && !is_task_field
        {
            flush_task(&mut current_name, &mut current_interval, &mut current_prompt, &mut tasks);
            break;
        }

        if !in_tasks_block {
            continue;
        }

        // New task entry: "- name:"
        if let Some(name) = trimmed.strip_prefix("- name:") {
            flush_task(&mut current_name, &mut current_interval, &mut current_prompt, &mut tasks);
            current_name = Some(name.trim().to_string());
        } else if let Some(val) = trimmed.strip_prefix("interval:") {
            current_interval = Some(val.trim().to_string());
        } else if let Some(val) = trimmed.strip_prefix("prompt:") {
            current_prompt = Some(val.trim().to_string());
        }
    }

    // Flush last task
    flush_task(&mut current_name, &mut current_interval, &mut current_prompt, &mut tasks);

    tasks
}

/// Check if HEARTBEAT.md content is effectively empty (only comments, whitespace, or markdown)
pub fn is_heartbeat_content_empty(content: &str) -> bool {
    content
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```"))
        .all(|l| l.is_empty())
}

/// Track when tasks were last executed for dedup
#[derive(Default)]
pub struct TaskDedupTracker {
    last_run: HashMap<String, std::time::Instant>,
}

impl TaskDedupTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a task is due to run based on its interval
    pub fn is_task_due(&self, task: &HeartbeatTask) -> bool {
        match self.last_run.get(&task.name) {
            Some(last) => last.elapsed() >= task.interval,
            None => true,
        }
    }

    /// Mark a task as having been executed
    pub fn mark_executed(&mut self, task_name: &str) {
        self.last_run.insert(task_name.to_string(), Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("2h30m"), Some(Duration::from_secs(9000)));
        assert_eq!(parse_duration(""), None);
    }

    #[test]
    fn test_parse_heartbeat_tasks_basic() {
        let content = r#"# HEARTBEAT.md

tasks:
  - name: email-check
    interval: 30m
    prompt: "Check for urgent emails"
  - name: log-review
    interval: 1h
    prompt: "Review error logs"
"#;
        let tasks = parse_heartbeat_tasks(content);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].name, "email-check");
        assert_eq!(tasks[0].interval, Duration::from_secs(1800));
        assert_eq!(tasks[1].name, "log-review");
    }

    #[test]
    fn test_parse_heartbeat_tasks_empty() {
        let content = "# Just a comment\nNothing here";
        let tasks = parse_heartbeat_tasks(content);
        assert!(tasks.is_empty());
    }
}
