//! Performance profiling utilities for Manta
//!
//! This module provides tools for profiling CPU usage, memory allocations,
//! and tracking performance metrics across the application.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::trace;

/// Performance metrics collector
#[derive(Debug, Default)]
pub struct Profiler {
    /// Named timers for tracking operation durations
    timers: Arc<RwLock<HashMap<String, Vec<Duration>>>>,
    /// Counters for tracking event occurrences
    counters: Arc<RwLock<HashMap<String, AtomicU64>>>,
    /// Memory tracking
    memory: Arc<RwLock<MemoryStats>>,
}

/// Memory statistics
#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    /// Peak memory usage in bytes
    pub peak_bytes: usize,
    /// Current memory usage in bytes (approximate)
    pub current_bytes: usize,
    /// Total allocations
    pub total_allocations: u64,
    /// Allocation history (for leak detection)
    pub allocation_history: Vec<AllocationRecord>,
}

/// Individual allocation record
#[derive(Debug, Clone)]
pub struct AllocationRecord {
    /// Size in bytes
    pub size: usize,
    /// Timestamp
    pub timestamp: Instant,
    /// Description
    pub description: String,
}

impl Profiler {
    /// Create a new profiler
    pub fn new() -> Self {
        Self::default()
    }

    /// Start timing an operation
    pub fn start_timer(&self, name: impl Into<String>) -> TimerGuard {
        TimerGuard::new(name.into(), self.timers.clone())
    }

    /// Record a duration for a named timer
    pub async fn record_duration(&self, name: impl Into<String>, duration: Duration) {
        let name = name.into();
        let mut timers = self.timers.write().await;
        timers.entry(name).or_default().push(duration);
    }

    /// Increment a counter
    pub async fn increment_counter(&self, name: impl Into<String>) {
        let name = name.into();
        let mut counters = self.counters.write().await;
        counters
            .entry(name)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Add to a counter by a specific amount
    pub async fn add_to_counter(&self, name: impl Into<String>, amount: u64) {
        let name = name.into();
        let mut counters = self.counters.write().await;
        counters
            .entry(name)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(amount, Ordering::Relaxed);
    }

    /// Record a memory allocation
    pub async fn record_allocation(&self, size: usize, description: impl Into<String>) {
        let mut memory = self.memory.write().await;
        memory.current_bytes += size;
        memory.total_allocations += 1;

        if memory.current_bytes > memory.peak_bytes {
            memory.peak_bytes = memory.current_bytes;
        }

        memory.allocation_history.push(AllocationRecord {
            size,
            timestamp: Instant::now(),
            description: description.into(),
        });

        // Keep history bounded
        if memory.allocation_history.len() > 1000 {
            memory.allocation_history.remove(0);
        }
    }

    /// Record memory deallocation
    pub async fn record_deallocation(&self, size: usize) {
        let mut memory = self.memory.write().await;
        memory.current_bytes = memory.current_bytes.saturating_sub(size);
    }

    /// Get timer statistics
    pub async fn get_timer_stats(&self, name: impl AsRef<str>) -> Option<TimerStats> {
        let timers = self.timers.read().await;
        timers.get(name.as_ref()).map(|durations| {
            let count = durations.len();
            let total: Duration = durations.iter().sum();
            let avg = total / count as u32;
            let min = *durations.iter().min().unwrap_or(&Duration::ZERO);
            let max = *durations.iter().max().unwrap_or(&Duration::ZERO);

            TimerStats {
                name: name.as_ref().to_string(),
                count,
                total,
                avg,
                min,
                max,
            }
        })
    }

    /// Get all timer statistics
    pub async fn get_all_timer_stats(&self) -> Vec<TimerStats> {
        let timers = self.timers.read().await;
        let mut stats = Vec::new();

        for name in timers.keys() {
            if let Some(stat) = self.get_timer_stats(name).await {
                stats.push(stat);
            }
        }

        stats.sort_by(|a, b| b.total.cmp(&a.total));
        stats
    }

    /// Get counter value
    pub async fn get_counter(&self, name: impl AsRef<str>) -> u64 {
        let counters = self.counters.read().await;
        counters
            .get(name.as_ref())
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Get all counters
    pub async fn get_all_counters(&self) -> HashMap<String, u64> {
        let counters = self.counters.read().await;
        counters
            .iter()
            .map(|(k, v)| (k.clone(), v.load(Ordering::Relaxed)))
            .collect()
    }

    /// Get memory stats
    pub async fn get_memory_stats(&self) -> MemoryStats {
        self.memory.read().await.clone()
    }

    /// Generate a performance report
    pub async fn generate_report(&self) -> PerformanceReport {
        PerformanceReport {
            timers: self.get_all_timer_stats().await,
            counters: self.get_all_counters().await,
            memory: self.get_memory_stats().await,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Reset all metrics
    pub async fn reset(&self) {
        let mut timers = self.timers.write().await;
        timers.clear();

        let mut counters = self.counters.write().await;
        counters.clear();

        let mut memory = self.memory.write().await;
        *memory = MemoryStats::default();
    }
}

/// Timer statistics
#[derive(Debug, Clone)]
pub struct TimerStats {
    /// Timer name
    pub name: String,
    /// Number of samples
    pub count: usize,
    /// Total duration
    pub total: Duration,
    /// Average duration
    pub avg: Duration,
    /// Minimum duration
    pub min: Duration,
    /// Maximum duration
    pub max: Duration,
}

/// Performance report
#[derive(Debug, Clone)]
pub struct PerformanceReport {
    /// Timer statistics
    pub timers: Vec<TimerStats>,
    /// Counter values
    pub counters: HashMap<String, u64>,
    /// Memory statistics
    pub memory: MemoryStats,
    /// Report timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl PerformanceReport {
    /// Format as human-readable string
    pub fn format(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!(
            "Performance Report - {}\n",
            self.timestamp.format("%Y-%m-%d %H:%M:%S UTC")
        ));
        output.push_str("========================================\n\n");

        // Timers
        if !self.timers.is_empty() {
            output.push_str("Timers:\n");
            for timer in &self.timers {
                output.push_str(&format!(
                    "  {}: {} calls, avg={:.2?}, min={:.2?}, max={:.2?}, total={:.2?}\n",
                    timer.name,
                    timer.count,
                    timer.avg,
                    timer.min,
                    timer.max,
                    timer.total
                ));
            }
            output.push('\n');
        }

        // Counters
        if !self.counters.is_empty() {
            output.push_str("Counters:\n");
            for (name, value) in &self.counters {
                output.push_str(&format!("  {}: {}\n", name, value));
            }
            output.push('\n');
        }

        // Memory
        output.push_str("Memory:\n");
        output.push_str(&format!(
            "  Current: {} bytes ({:.2} MB)\n",
            self.memory.current_bytes,
            self.memory.current_bytes as f64 / 1_048_576.0
        ));
        output.push_str(&format!(
            "  Peak: {} bytes ({:.2} MB)\n",
            self.memory.peak_bytes,
            self.memory.peak_bytes as f64 / 1_048_576.0
        ));
        output.push_str(&format!("  Total allocations: {}\n", self.memory.total_allocations));

        output
    }
}

/// RAII guard for timing operations
pub struct TimerGuard {
    name: String,
    start: Instant,
    timers: Arc<RwLock<HashMap<String, Vec<Duration>>>>,
}

impl TimerGuard {
    fn new(name: String, timers: Arc<RwLock<HashMap<String, Vec<Duration>>>>) -> Self {
        Self {
            name,
            start: Instant::now(),
            timers,
        }
    }
}

impl Drop for TimerGuard {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        let name = self.name.clone();
        let timers = self.timers.clone();

        trace!("Timer '{}' completed in {:.2?}", name, duration);

        // Spawn async task to record the duration
        tokio::spawn(async move {
            let mut timers = timers.write().await;
            timers.entry(name).or_default().push(duration);
        });
    }
}

/// Global profiler instance
static GLOBAL_PROFILER: std::sync::OnceLock<Profiler> = std::sync::OnceLock::new();

/// Get the global profiler instance
pub fn global_profiler() -> &'static Profiler {
    GLOBAL_PROFILER.get_or_init(Profiler::new)
}

/// Convenience macro to time a block
#[macro_export]
macro_rules! time_block {
    ($name:expr) => {
        let _timer = $crate::utils::profiling::global_profiler().start_timer($name);
    };
}

/// Convenience macro to count an event
#[macro_export]
macro_rules! count_event {
    ($name:expr) => {
        tokio::spawn(async move {
            $crate::utils::profiling::global_profiler().increment_counter($name).await;
        });
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_profiler_counters() {
        let profiler = Profiler::new();

        profiler.increment_counter("test").await;
        profiler.increment_counter("test").await;
        profiler.increment_counter("test").await;

        let count = profiler.get_counter("test").await;
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_profiler_memory() {
        let profiler = Profiler::new();

        profiler.record_allocation(1024, "test allocation").await;
        profiler.record_allocation(2048, "another allocation").await;

        let stats = profiler.get_memory_stats().await;
        assert_eq!(stats.current_bytes, 3072);
        assert_eq!(stats.peak_bytes, 3072);
        assert_eq!(stats.total_allocations, 2);

        profiler.record_deallocation(1024).await;
        let stats = profiler.get_memory_stats().await;
        assert_eq!(stats.current_bytes, 2048);
    }

    #[tokio::test]
    async fn test_performance_report() {
        let profiler = Profiler::new();

        profiler.increment_counter("requests").await;
        profiler.record_allocation(1024, "test").await;

        let report = profiler.generate_report().await;
        assert!(!report.counters.is_empty());
        assert_eq!(report.counters.get("requests"), Some(&1));
    }
}
