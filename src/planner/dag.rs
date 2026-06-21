//! DAG scheduler — topological sort and execution ordering.
//!
//! Ensures tasks are executed only after all their dependencies have
//! completed successfully.  Independent tasks can run in parallel.

use std::collections::{HashMap, HashSet, VecDeque};

use super::{Plan, TaskId};

/// Detects cycles and computes topological order for a plan.
#[derive(Debug)]
pub struct DagScheduler {
    /// Cached topological order (computed once and reused).
    order: Vec<TaskId>,
}

/// Result of validating a plan's dependency graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagValidation {
    /// The DAG is valid and ready for execution.
    Valid,
    /// A task depends on itself.
    SelfDependency { task: TaskId },
    /// A task depends on another task that does not exist.
    MissingDependency { task: TaskId, missing: TaskId },
    /// A cycle was detected in the dependency graph.
    Cycle { tasks: Vec<TaskId> },
}

impl DagScheduler {
    /// Create a scheduler from a plan.
    ///
    /// Returns `Err(DagValidation)` if the plan contains cycles or missing
    /// dependencies.
    pub fn from_plan(plan: &Plan) -> Result<Self, DagValidation> {
        Self::validate(plan)?;
        let order = Self::topological_sort(plan);
        Ok(Self { order })
    }

    /// Validate the plan's dependency graph.
    pub fn validate(plan: &Plan) -> Result<(), DagValidation> {
        // Check for self-dependencies and missing dependencies.
        for (id, task) in &plan.tasks {
            for dep in &task.dependencies {
                if dep == id {
                    return Err(DagValidation::SelfDependency { task: id.clone() });
                }
                if !plan.tasks.contains_key(dep) {
                    return Err(DagValidation::MissingDependency {
                        task: id.clone(),
                        missing: dep.clone(),
                    });
                }
            }
        }

        // Check for cycles using Kahn's algorithm.
        let mut in_degree: HashMap<TaskId, usize> = HashMap::new();
        let mut adj: HashMap<TaskId, Vec<TaskId>> = HashMap::new();

        for id in plan.tasks.keys() {
            in_degree.insert(id.clone(), 0);
        }

        for (id, task) in &plan.tasks {
            for dep in &task.dependencies {
                adj.entry(dep.clone()).or_default().push(id.clone());
                *in_degree.get_mut(id).unwrap() += 1;
            }
        }

        let mut queue: VecDeque<TaskId> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut visited = 0usize;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            if let Some(children) = adj.get(&id) {
                for child in children {
                    let d = in_degree.get_mut(child).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }

        if visited != plan.tasks.len() {
            // Find tasks that are part of a cycle.
            let cycle_tasks: Vec<TaskId> = in_degree
                .iter()
                .filter(|(_, d)| **d > 0)
                .map(|(id, _)| id.clone())
                .collect();
            return Err(DagValidation::Cycle { tasks: cycle_tasks });
        }

        Ok(())
    }

    /// Compute a topological sort of the plan's tasks.
    fn topological_sort(plan: &Plan) -> Vec<TaskId> {
        let mut in_degree: HashMap<TaskId, usize> = HashMap::new();
        let mut adj: HashMap<TaskId, Vec<TaskId>> = HashMap::new();

        for id in plan.tasks.keys() {
            in_degree.insert(id.clone(), 0);
        }

        for (id, task) in &plan.tasks {
            for dep in &task.dependencies {
                adj.entry(dep.clone()).or_default().push(id.clone());
                *in_degree.get_mut(id).unwrap() += 1;
            }
        }

        let mut queue: VecDeque<TaskId> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(id, _)| id.clone())
            .collect();

        let mut order = Vec::new();
        while let Some(id) = queue.pop_front() {
            order.push(id.clone());
            if let Some(children) = adj.get(&id) {
                for child in children {
                    let d = in_degree.get_mut(child).unwrap();
                    *d -= 1;
                    if *d == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }

        order
    }

    /// Return the full topological order.
    pub fn order(&self) -> &[TaskId] {
        &self.order
    }

    /// Return tasks that have no remaining uncompleted dependencies.
    ///
    /// Given a set of completed task IDs, returns all tasks whose
    /// dependencies are all in the completed set.
    pub fn next_ready(
        &self,
        plan: &Plan,
        completed: &HashSet<TaskId>,
        failed: &HashSet<TaskId>,
    ) -> Vec<TaskId> {
        plan.tasks
            .values()
            .filter(|t| {
                // Not yet started
                t.status == super::TaskStatus::Pending
                    // All dependencies completed
                    && t.dependencies.iter().all(|dep| completed.contains(dep))
                    // No dependencies failed
                    && t.dependencies.iter().all(|dep| !failed.contains(dep))
            })
            .map(|t| t.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::DesktopAction;
    use crate::planner::{Plan, Task};

    #[test]
    fn test_valid_dag() {
        let mut plan = Plan::new("deploy");
        plan.add_task(Task::new(
            "build",
            "Build project",
            DesktopAction::Wait { milliseconds: 10 },
        ));
        plan.add_task(
            Task::new("test", "Run tests", DesktopAction::Wait { milliseconds: 10 })
                .depends_on("build"),
        );
        plan.add_task(
            Task::new("deploy", "Deploy", DesktopAction::Wait { milliseconds: 10 })
                .depends_on("test"),
        );

        let scheduler = DagScheduler::from_plan(&plan).unwrap();
        assert_eq!(scheduler.order(), vec!["build", "test", "deploy"]);
    }

    #[test]
    fn test_parallel_tasks() {
        let mut plan = Plan::new("parallel");
        plan.add_task(Task::new("setup", "Setup", DesktopAction::Wait { milliseconds: 10 }));
        plan.add_task(
            Task::new("a", "Task A", DesktopAction::Wait { milliseconds: 10 }).depends_on("setup"),
        );
        plan.add_task(
            Task::new("b", "Task B", DesktopAction::Wait { milliseconds: 10 }).depends_on("setup"),
        );
        plan.add_task(
            Task::new("c", "Task C", DesktopAction::Wait { milliseconds: 10 }).depends_on("setup"),
        );
        plan.add_task(
            Task::new("finish", "Finish", DesktopAction::Wait { milliseconds: 10 })
                .depends_on("a")
                .depends_on("b")
                .depends_on("c"),
        );

        let scheduler = DagScheduler::from_plan(&plan).unwrap();
        assert_eq!(scheduler.order[0], "setup");
        assert_eq!(scheduler.order[4], "finish");
        // a, b, c can be in any order among themselves
        let mid: HashSet<String> = scheduler.order[1..4].iter().cloned().collect();
        assert!(mid.contains("a"));
        assert!(mid.contains("b"));
        assert!(mid.contains("c"));
    }

    #[test]
    fn test_self_dependency_detected() {
        let mut plan = Plan::new("bad");
        plan.add_task(
            Task::new("a", "Task A", DesktopAction::Wait { milliseconds: 10 }).depends_on("a"),
        );

        let err = DagScheduler::from_plan(&plan).unwrap_err();
        assert!(matches!(err, DagValidation::SelfDependency { task } if task == "a"));
    }

    #[test]
    fn test_missing_dependency_detected() {
        let mut plan = Plan::new("bad");
        plan.add_task(
            Task::new("a", "Task A", DesktopAction::Wait { milliseconds: 10 })
                .depends_on("missing"),
        );

        let err = DagScheduler::from_plan(&plan).unwrap_err();
        assert!(
            matches!(err, DagValidation::MissingDependency { task, missing } if task == "a" && missing == "missing")
        );
    }

    #[test]
    fn test_cycle_detected() {
        let mut plan = Plan::new("bad");
        plan.add_task(
            Task::new("a", "Task A", DesktopAction::Wait { milliseconds: 10 }).depends_on("c"),
        );
        plan.add_task(
            Task::new("b", "Task B", DesktopAction::Wait { milliseconds: 10 }).depends_on("a"),
        );
        plan.add_task(
            Task::new("c", "Task C", DesktopAction::Wait { milliseconds: 10 }).depends_on("b"),
        );

        let err = DagScheduler::from_plan(&plan).unwrap_err();
        assert!(matches!(err, DagValidation::Cycle { .. }));
    }

    #[test]
    fn test_next_ready() {
        let mut plan = Plan::new("test");
        plan.add_task(Task::new("a", "A", DesktopAction::Wait { milliseconds: 10 }));
        plan.add_task(
            Task::new("b", "B", DesktopAction::Wait { milliseconds: 10 }).depends_on("a"),
        );
        plan.add_task(
            Task::new("c", "C", DesktopAction::Wait { milliseconds: 10 }).depends_on("a"),
        );
        plan.add_task(
            Task::new("d", "D", DesktopAction::Wait { milliseconds: 10 })
                .depends_on("b")
                .depends_on("c"),
        );

        let scheduler = DagScheduler::from_plan(&plan).unwrap();
        let mut completed = HashSet::new();
        let failed = HashSet::new();
        let ready = scheduler.next_ready(&plan, &completed, &failed);
        assert_eq!(ready, vec!["a"]);

        plan.complete_task("a", "done".to_string());
        completed.insert("a".to_string());
        let mut ready = scheduler.next_ready(&plan, &completed, &failed);
        ready.sort();
        assert_eq!(ready, vec!["b", "c"]);

        plan.complete_task("b", "done".to_string());
        plan.complete_task("c", "done".to_string());
        completed.insert("b".to_string());
        completed.insert("c".to_string());
        let ready = scheduler.next_ready(&plan, &completed, &failed);
        assert_eq!(ready, vec!["d"]);
    }
}
