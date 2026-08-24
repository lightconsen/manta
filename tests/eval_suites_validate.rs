//! Deterministic validation of the evals/ YAML assets.
//!
//! Runs in the standard `cargo test` CI step so a broken suite manifest or
//! task file fails fast before anyone burns LLM tokens on `eval run --full`.

use std::path::{Path, PathBuf};

fn evals_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("evals")
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    if !dir.is_dir() {
        return;
    }
    for entry in std::fs::read_dir(dir).expect("read evals dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_yaml(&path, out);
        } else if path.extension().is_some_and(|e| e == "yaml" || e == "yml") {
            out.push(path);
        }
    }
}

/// Every suite manifest under evals/suites/ must load and resolve its task
/// references (registry.yaml is a category index, not a suite manifest).
#[test]
fn suite_manifests_load() {
    let mut files = Vec::new();
    collect_yaml(&evals_dir().join("suites"), &mut files);
    assert!(!files.is_empty(), "no suite manifests found");

    for f in files {
        let stem = f.file_stem().unwrap().to_string_lossy().to_string();
        if stem == "registry" {
            let content = std::fs::read_to_string(&f).expect("read registry.yaml");
            assert!(content.contains("suites:"), "registry.yaml lost its category index shape");
            continue;
        }
        let suite = syscity::eval::load_suite(&f, &stem)
            .unwrap_or_else(|e| panic!("suite {} failed to load: {e}", f.display()));
        assert!(suite.trials >= 1, "{}: trials must be >= 1", suite.id);
        assert!(
            (0.0..=1.0).contains(&suite.min_pass_rate),
            "{}: min_pass_rate must be within [0, 1]",
            suite.id
        );
    }
}

/// Every task file in the category directories must parse and contain tasks.
#[test]
fn task_files_load() {
    for sub in ["capability", "regression", "adversarial", "skills"] {
        let dir = evals_dir().join(sub);
        let mut files = Vec::new();
        collect_yaml(&dir, &mut files);
        assert!(!files.is_empty(), "no task files under {}", dir.display());

        for f in files {
            let loaded = syscity::eval::load_tasks(&f)
                .unwrap_or_else(|e| panic!("task file {} failed: {e}", f.display()));
            assert!(
                !loaded.tasks.is_empty(),
                "{}: parsed successfully but contains no tasks",
                f.display()
            );
        }
    }
}

/// The Critic calibration set must parse and contain known-answer cases.
#[test]
fn calibration_cases_load() {
    let path = evals_dir().join("calibration").join("default.yaml");
    let cases = syscity::eval::load_calibration_cases(&path)
        .unwrap_or_else(|e| panic!("calibration file failed: {e}"));
    assert!(!cases.is_empty(), "calibration set is empty");
}
