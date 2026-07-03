//! Startup recovery — scan for incomplete plans and offer to resume them.
//!
//! Called during daemon / CLI startup to detect plans that were interrupted
//! by a crash or shutdown and give the user the option to continue.

use tracing::{info, warn};

use tokio::io::AsyncBufReadExt;

use crate::planner::{GoalPlanner, PlanResult, TaskStateStore};

/// Result of a startup recovery check.
#[derive(Debug)]
pub enum RecoveryOutcome {
    /// No incomplete plans were found.
    NothingToResume,
    /// User chose to resume one or more plans.
    Resumed(Vec<PlanResult>),
    /// User declined to resume.
    Declined,
}

/// Scan the state store for incomplete plans and optionally resume them.
///
/// This function is intended to be called once at application startup
/// (e.g. inside the daemon's `run_foreground`).  It:
///
/// 1. Queries the [`TaskStateStore`] for all plans with `completed_at IS NULL`.
/// 2. Prints a summary to stdout so the user can see what was interrupted.
/// 3. If `auto_resume` is `true`, resumes the most recent incomplete plan
///    without prompting (headless / server mode).
/// 4. If `auto_resume` is `false` and **stdin is a TTY**, asks the user whether
///    to resume.  In non-TTY environments the prompt is skipped and the
///    function returns [`RecoveryOutcome::Declined`].
///
/// # Example
/// ```rust,no_run
/// # async fn example() -> syscity::Result<()> {
/// use std::sync::Arc;
///
/// use syscity::planner::{check_startup_recovery, TaskStateStore};
///
/// let store = TaskStateStore::new("sqlite://~/.syscity/planner.db").await?;
/// // let planner = ... ;
/// // check_startup_recovery(&store, &planner, false).await?;
/// # Ok(())
/// # }
/// ```
pub async fn check_startup_recovery(
    store: &TaskStateStore,
    planner: &GoalPlanner,
    auto_resume: bool,
) -> crate::Result<RecoveryOutcome> {
    let summaries = store.load_plan_summaries().await?;

    if summaries.is_empty() {
        info!("No incomplete plans found — nothing to resume");
        return Ok(RecoveryOutcome::NothingToResume);
    }

    println!("\n📋 Interrupted plans detected:");
    println!("{:-<60}", "");
    for (i, s) in summaries.iter().enumerate() {
        println!(
            "  {}. {} (created: {})",
            i + 1,
            s.goal,
            s.created_at.split('T').next().unwrap_or(&s.created_at)
        );
        println!(
            "     Progress: {}/{} completed, {} failed, {} pending",
            s.completed_tasks, s.total_tasks, s.failed_tasks, s.pending_tasks
        );
    }
    println!("{:-<60}\n", "");

    if auto_resume {
        info!("Auto-resume enabled — resuming most recent plan");
        if let Some(s) = summaries.into_iter().next() {
            match planner.resume_plan(&s.id).await {
                Ok(Some(result)) => {
                    println!("✅ Plan '{}' resumed: {}", s.goal, result.message);
                    Ok(RecoveryOutcome::Resumed(vec![result]))
                }
                Ok(None) => {
                    warn!("Plan '{}' disappeared during resume", s.id);
                    Ok(RecoveryOutcome::Declined)
                }
                Err(e) => {
                    warn!("Failed to resume plan '{}': {}", s.id, e);
                    Ok(RecoveryOutcome::Declined)
                }
            }
        } else {
            Ok(RecoveryOutcome::NothingToResume)
        }
    } else {
        // Interactive prompt only when stdin is a TTY.
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() {
            info!("Non-interactive mode — skipping resume prompt");
            return Ok(RecoveryOutcome::Declined);
        }

        println!("Resume the most recent plan? [y/N]");
        let reader = tokio::io::BufReader::new(tokio::io::stdin());
        let mut lines = reader.lines();
        match lines.next_line().await {
            Ok(Some(line)) if line.trim().eq_ignore_ascii_case("y") => {
                let s = &summaries[0];
                info!("User chose to resume plan '{}'", s.id);
                match planner.resume_plan(&s.id).await {
                    Ok(Some(result)) => {
                        println!("✅ Plan '{}' resumed: {}", s.goal, result.message);
                        Ok(RecoveryOutcome::Resumed(vec![result]))
                    }
                    Ok(None) => {
                        println!("⚠️  Plan no longer exists");
                        Ok(RecoveryOutcome::Declined)
                    }
                    Err(e) => {
                        println!("❌ Failed to resume plan: {}", e);
                        Ok(RecoveryOutcome::Declined)
                    }
                }
            }
            _ => {
                info!("User declined to resume interrupted plans");
                Ok(RecoveryOutcome::Declined)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recovery_outcome_debug() {
        let o = RecoveryOutcome::NothingToResume;
        assert!(format!("{:?}", o).contains("NothingToResume"));
    }
}
