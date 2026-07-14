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
                )
                .await
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
                    println!("  ✓ {} (suite)", path.file_name().unwrap().to_string_lossy());
                }
                Err(e) => {
                    errors.push(format!("{}: {}", path.display(), e));
                }
            }
        } else {
            // Validate task file — parse as EvalTask
            match eval::load_tasks(path) {
                Ok(tasks) => {
                    total_files += 1;
                    total_tasks += tasks.len();
                    let name = path.file_name().unwrap().to_string_lossy();
                    println!("  ✓ {} ({} tasks)", name, tasks.len());
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
                f(&path).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
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
                f(&path).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
            }
        }
    }
    Ok(())
}
