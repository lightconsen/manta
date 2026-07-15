//! Eval CLI subcommand — load, validate, and run evaluation suites.
//!
//! ```bash
//! syscity eval list               # List all available suites
//! syscity eval validate           # Validate all YAML task files
//! syscity eval run <suite>        # Load + dry-run a suite
//! syscity eval run <suite> --full # Run a suite with a live Agent (standalone, no daemon needed)
//! ```

use std::path::PathBuf;

use clap::Subcommand;

use crate::config::Config;
use crate::eval;
use crate::Result;

/// Eval subcommands
#[derive(Debug, Subcommand)]
pub enum EvalCommands {
    /// List all available evaluation suites
    List {
        /// Path to evals directory (defaults to `./evals`)
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
    /// Validate all YAML task files in the evals directory
    Validate {
        /// Path to evals directory (defaults to `./evals`)
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
    /// Load and run an evaluation suite
    Run {
        /// Suite name (e.g. "ci_smoke", "capability", "regression")
        suite: String,
        /// Path to evals directory (defaults to `./evals`)
        #[arg(short, long)]
        dir: Option<PathBuf>,
        /// Actually execute tasks (standalone, no daemon needed)
        #[arg(long)]
        full: bool,
        /// Number of trials per task
        #[arg(short, long)]
        trials: Option<usize>,
        /// LLM provider (e.g. "anthropic", "openai")
        #[arg(long)]
        provider: Option<String>,
        /// Model name (e.g. "claude-sonnet-4-20250514")
        #[arg(long)]
        model: Option<String>,
        /// API key (overrides env var / config file)
        #[arg(long)]
        api_key: Option<String>,
        /// API base URL (for custom/self-hosted providers)
        #[arg(long)]
        base_url: Option<String>,
        /// Show detailed skill evaluation breakdown per dimension
        #[arg(long)]
        skill_breakdown: bool,
        /// Collect badcases from failed trials into evals/badcases/
        #[arg(long)]
        collect_badcases: bool,
    },
    /// List collected badcases
    BadcaseList {
        /// Path to evals directory (defaults to `./evals`)
        #[arg(short, long)]
        dir: Option<PathBuf>,
        /// Show RCA details
        #[arg(long)]
        verbose: bool,
    },
    /// Show details of a specific badcase file
    BadcaseShow {
        /// Badcase task file stem (e.g. "tool_selection")
        filter: String,
        /// Path to evals directory (defaults to `./evals`)
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
}

impl EvalCommands {
    pub async fn run(&self, config: &Config) -> Result<()> {
        match self {
            Self::List { dir } => cmd_list(dir.clone()).await,
            Self::Validate { dir } => cmd_validate(dir.clone()).await,
            Self::Run {
                suite,
                dir,
                full,
                trials,
                provider,
                model,
                api_key,
                base_url,
                skill_breakdown,
                collect_badcases,
            } => {
                cmd_run(
                    config,
                    suite.clone(),
                    dir.clone(),
                    *full,
                    *trials,
                    provider.clone(),
                    model.clone(),
                    api_key.clone(),
                    base_url.clone(),
                    *skill_breakdown,
                    *collect_badcases,
                )
                .await
            }
            Self::BadcaseList { dir, verbose } => {
                cmd_badcase_list(dir.clone(), *verbose).await
            }
            Self::BadcaseShow { filter, dir } => {
                cmd_badcase_show(filter.clone(), dir.clone()).await
            }
        }
    }
}

/// `eval list` — show all available suites.
async fn cmd_list(dir: Option<PathBuf>) -> Result<()> {
    let evals_dir = dir.unwrap_or_else(eval::default_evals_dir);
    let suites = eval::list_suites(&evals_dir)?;

    if suites.is_empty() {
        println!("No suites found in {:?}", evals_dir.join("suites"));
        println!("  (expected .yaml files in evals/suites/)");
        return Ok(());
    }

    println!("Available suites:");
    for (name, display) in &suites {
        println!("  {:20}  {}", name, display);
    }
    Ok(())
}

/// `eval validate` — check all YAML files parse correctly.
async fn cmd_validate(dir: Option<PathBuf>) -> Result<()> {
    let evals_dir = dir.unwrap_or_else(eval::default_evals_dir);
    let mut total_files = 0usize;
    let mut total_tasks = 0usize;
    let mut errors = Vec::new();

    // Walk YAML files in evals/ recursively
    collect_yaml_files(&evals_dir, &mut Vec::new(), &mut |path| {
        // Check if it's a suite manifest (in suites/) or a task file
        let is_suite = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n == "suites")
            .unwrap_or(false);

        if is_suite {
            // Validate suite manifest structure
            let content = std::fs::read_to_string(path)
                .map_err(crate::error::SyscityError::Io)?;
            match serde_yml::from_str::<serde_yml::Value>(&content) {
                Ok(_) => {
                    total_files += 1;
                    if let Some(name) = path.file_name() {
                        println!("  ✓ {} (suite)", name.to_string_lossy());
                    }
                }
                Err(e) => {
                    errors.push(format!("{}: {}", path.display(), e));
                }
            }
        } else {
            // Validate task file — parse as EvalTask
            match eval::load_tasks(path) {
                Ok(loaded) => {
                    total_files += 1;
                    total_tasks += loaded.tasks.len();
                    if let Some(name) = path.file_name() {
                        println!("  ✓ {} ({} tasks)", name.to_string_lossy(), loaded.tasks.len());
                    }
                }
                Err(e) => {
                    errors.push(format!("{}: {}", path.display(), e));
                }
            }
        }
        Ok::<_, crate::error::SyscityError>(())
    })?;

    println!();
    if errors.is_empty() {
        println!(
            "✅ All {} YAML files valid ({} tasks across {} files)",
            total_files, total_tasks, total_files
        );
    } else {
        for err in &errors {
            eprintln!("❌ {}", err);
        }
        eprintln!(
            "❌ {}/{} files have errors",
            errors.len(),
            total_files + errors.len()
        );
    }

    Ok(())
}

/// `eval run <suite>` — load and optionally execute a suite.
#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    config: &Config,
    suite: String,
    dir: Option<PathBuf>,
    full: bool,
    trials: Option<usize>,
    provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    skill_breakdown: bool,
    collect_badcases: bool,
) -> Result<()> {
    let evals_dir = dir.unwrap_or_else(eval::default_evals_dir);
    let manifest_path = evals_dir.join("suites").join(format!("{}.yaml", suite));

    if !manifest_path.exists() {
        eprintln!("❌ Suite '{}' not found at {:?}", suite, manifest_path);
        eprintln!("   Run `syscity eval list` to see available suites.");
        return Ok(());
    }

    let eval_suite = eval::load_suite(&manifest_path, &suite)?;

    if full {
        // Standalone eval mode (no daemon needed)
        return eval::standalone::run_standalone_suite(
            config,
            &suite,
            Some(evals_dir),
            trials,
            provider,
            model,
            api_key,
            base_url,
            skill_breakdown,
            collect_badcases,
        )
        .await;
    }

    // Dry-run: print suite details
    println!("═══ Eval Suite: {} ═══", eval_suite.name);
    println!("  ID:       {}", eval_suite.id);
    println!("  Category: {:?}", eval_suite.category);
    println!("  Tasks:    {}", eval_suite.tasks.len());
    println!("  Min pass: {:.0}%", eval_suite.min_pass_rate * 100.0);
    println!("  Trials:   {}", eval_suite.trials);
    println!();

    for task in &eval_suite.tasks {
        println!("  Task: {} — {}", task.id, task.description);
        println!("    Input: {}", task.input.chars().take(80).collect::<String>());
        println!("    Conditions: {}", task.conditions.len());
        if task.criteria.is_some() {
            println!("    Criteria: yes");
        }
        if !task.setup.is_empty() {
            println!("    Setup: {} commands", task.setup.len());
        }
        if !task.cleanup.is_empty() {
            println!("    Cleanup: {} commands", task.cleanup.len());
        }
        println!();
    }

    println!(
        "✓ Suite loaded successfully. {} tasks, {} trials each.",
        eval_suite.tasks.len(),
        eval_suite.trials
    );
    println!("  Use --full flag to execute (standalone, no daemon needed).");

    Ok(())
}

/// `eval badcase-list` — show collected badcases.
async fn cmd_badcase_list(dir: Option<PathBuf>, verbose: bool) -> Result<()> {
    let evals_dir = dir.unwrap_or_else(eval::default_evals_dir);
    let badcases_dir = evals_dir.join("badcases");

    if !badcases_dir.is_dir() {
        println!("No badcases directory found at {:?}", badcases_dir);
        println!("  Run `syscity eval run <suite> --full --collect-badcases` to collect badcases.");
        return Ok(());
    }

    let mut files: Vec<_> = std::fs::read_dir(&badcases_dir)
        .map_err(crate::error::SyscityError::Io)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "yaml").unwrap_or(false))
        .collect();
    files.sort_by_key(|e| e.path());

    if files.is_empty() {
        println!("No badcase files found in {:?}", badcases_dir);
        return Ok(());
    }

    println!("Badcases in {:?}:", badcases_dir);
    for entry in &files {
        let path = entry.path();
        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match eval::load_tasks(&path) {
            Ok(loaded) => {
                let badcase_count = loaded.tasks.iter().filter(|t| t.failure_reason.is_some()).count();
                println!("  {:30} {} tasks ({} badcases)", file_name, loaded.tasks.len(), badcase_count);
                if verbose {
                    for task in &loaded.tasks {
                        let reason = task.failure_reason.as_deref().unwrap_or("none");
                        println!("      {} — {}", task.id, reason);
                    }
                }
            }
            Err(e) => {
                println!("  {:30} error: {}", file_name, e);
            }
        }
    }

    Ok(())
}

/// `eval badcase-show` — display a specific badcase file.
async fn cmd_badcase_show(filter: String, dir: Option<PathBuf>) -> Result<()> {
    let evals_dir = dir.unwrap_or_else(eval::default_evals_dir);
    let badcases_dir = evals_dir.join("badcases");
    let file_path = badcases_dir.join(format!("{}.yaml", filter));

    if !file_path.exists() {
        // Try fuzzy match
        let mut found = false;
        if badcases_dir.is_dir() {
            for entry in std::fs::read_dir(&badcases_dir).map_err(crate::error::SyscityError::Io)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map(|e| e == "yaml").unwrap_or(false) {
                    if let Some(stem) = path.file_stem() {
                        if stem.to_string_lossy().contains(&filter) {
                            let content = std::fs::read_to_string(&path)
                                .map_err(crate::error::SyscityError::Io)?;
                            println!("=== {} ===\n{}", path.display(), content);
                            found = true;
                            break;
                        }
                    }
                }
            }
        }
        if !found {
            eprintln!("❌ No badcase file matching '{}' found in {:?}", filter, badcases_dir);
        }
        return Ok(());
    }

    let content = std::fs::read_to_string(&file_path)
        .map_err(crate::error::SyscityError::Io)?;
    println!("=== {} ===\n{}", file_path.display(), content);
    Ok(())
}

/// Recursively collect all `.yaml` files under a directory and call `f` on each.
fn collect_yaml_files<F>(dir: &std::path::Path, dirs: &mut Vec<std::path::PathBuf>, f: &mut F) -> std::io::Result<()>
where
    F: FnMut(&std::path::Path) -> std::result::Result<(), crate::error::SyscityError>,
{
    use std::io;
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path.clone());
            } else if path.extension().map(|e| e == "yaml").unwrap_or(false) {
                f(&path).map_err(|e| io::Error::other(e.to_string()))?;
            }
        }
    }
    while let Some(subdir) = dirs.pop() {
        for entry in std::fs::read_dir(&subdir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().map(|e| e == "yaml").unwrap_or(false) {
                f(&path).map_err(|e| io::Error::other(e.to_string()))?;
            }
        }
    }
    Ok(())
}
