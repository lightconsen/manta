//! Cron Scheduler for Syscity
//!
//! Provides production-grade scheduled task execution with timer-based
//! scheduling, retry logic, crash recovery, and run history logging.
//!
//! # Architecture
//!
//! - **CronScheduler** (`cron`): Primary scheduler used by Gateway and CronTool
//! - **CronJob** (`cron::CronJob`): Job definition (schedule, target, delivery)
//! - **CronTool** (`crate::tools::cron_tool`): AI-facing tool interface

#[allow(clippy::module_inception)]
pub mod cron;
