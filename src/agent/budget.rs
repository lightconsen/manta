//! Iteration Budget for Agent Loop
//!
//! This module implements a shared iteration counter to prevent runaway
//! execution and control API costs. Inspired by Hermes-Agent.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Shared iteration budget for agent loops
#[derive(Debug, Clone)]
pub struct IterationBudget {
    /// The remaining iterations
    remaining: Arc<AtomicUsize>,
    /// The maximum allowed iterations
    max: usize,
}

impl IterationBudget {
    /// Create a new budget with specified maximum iterations
    pub fn new(max: usize) -> Self {
        Self {
            remaining: Arc::new(AtomicUsize::new(max)),
            max,
        }
    }

    /// Get the maximum iterations
    pub fn max(&self) -> usize {
        self.max
    }

    /// Get remaining iterations
    pub fn remaining(&self) -> usize {
        self.remaining.load(Ordering::Relaxed)
    }

    /// Check if budget is exhausted
    pub fn is_exhausted(&self) -> bool {
        self.remaining.load(Ordering::Relaxed) == 0
    }

    /// Consume one iteration, returns true if successful
    pub fn consume(&self) -> bool {
        let current = self.remaining.load(Ordering::Relaxed);
        if current == 0 {
            return false;
        }
        self.remaining
            .compare_exchange(current, current - 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    /// Get a child budget that shares the same counter
    pub fn child(&self) -> Self {
        Self {
            remaining: Arc::clone(&self.remaining),
            max: self.max,
        }
    }

    /// Reset the budget to maximum
    pub fn reset(&self) {
        self.remaining.store(self.max, Ordering::Relaxed);
    }

    /// Get budget summary for display
    pub fn summary(&self) -> String {
        let remaining = self.remaining();
        let used = self.max - remaining;
        format!("Budget: {}/{} used, {} remaining", used, self.max, remaining)
    }
}

impl Default for IterationBudget {
    fn default() -> Self {
        Self::new(50) // Default 50 iterations
    }
}

/// Budget exhaustion handler
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetExhaustionAction {
    /// Return an error
    Error,
    /// Return current results
    ReturnPartial,
    /// Ask user for more budget
    AskUser,
}

/// Configuration for iteration budget
#[derive(Debug, Clone)]
pub struct BudgetConfig {
    /// Maximum iterations
    pub max_iterations: usize,
    /// Action when budget is exhausted
    pub exhaustion_action: BudgetExhaustionAction,
    /// Warning threshold (percentage)
    pub warning_threshold: f32,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            exhaustion_action: BudgetExhaustionAction::ReturnPartial,
            warning_threshold: 0.8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_creation() {
        let budget = IterationBudget::new(10);
        assert_eq!(budget.max(), 10);
        assert_eq!(budget.remaining(), 10);
        assert!(!budget.is_exhausted());
    }

    #[test]
    fn test_budget_consume() {
        let budget = IterationBudget::new(3);
        assert!(budget.consume());
        assert_eq!(budget.remaining(), 2);
        assert!(budget.consume());
        assert_eq!(budget.remaining(), 1);
        assert!(budget.consume());
        assert_eq!(budget.remaining(), 0);
        assert!(budget.is_exhausted());
        assert!(!budget.consume());
    }

    #[test]
    fn test_child_budget() {
        let parent = IterationBudget::new(10);
        let child = parent.child();

        // Child shares the same counter
        child.consume();
        assert_eq!(parent.remaining(), 9);

        parent.consume();
        assert_eq!(child.remaining(), 8);
    }

    #[test]
    fn test_budget_reset() {
        let budget = IterationBudget::new(5);
        budget.consume();
        budget.consume();
        assert_eq!(budget.remaining(), 3);

        budget.reset();
        assert_eq!(budget.remaining(), 5);
    }
}
