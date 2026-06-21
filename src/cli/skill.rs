//! Skill management commands for Syscity

use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::OutputFormat;
use crate::error::{Result, SyscityError};
use crate::skills::{install_all, SkillFile};

/// Default daemon base URL.
const DAEMON_URL: &str = "http://127.0.0.1:18080";

#[derive(Debug, Subcommand)]
pub enum SkillCommands {
    /// List all available skills
    List {
        /// Show all skills including ineligible ones
        #[arg(short, long)]
        all: bool,
        /// Output format
        #[arg(short, long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Show detailed information about a skill
    Info {
        /// Skill name
        name: String,
    },
    /// Install a skill from a directory, git repo, or registry
    Install {
        /// Path to skill directory or git URL, or skill name when --registry is
        /// set
        source: String,
        /// Skill name (optional, defaults to directory name)
        #[arg(short, long)]
        name: Option<String>,
        /// Install from the remote skill registry instead of local/git
        #[arg(short, long)]
        registry: bool,
    },
    /// Uninstall a skill
    Uninstall {
        /// Skill name
        name: String,
        /// Skip confirmation
        #[arg(short, long)]
        force: bool,
    },
    /// Enable a skill
    Enable {
        /// Skill name
        name: String,
    },
    /// Disable a skill
    Disable {
        /// Skill name
        name: String,
    },
    /// Install dependencies for a skill
    Setup {
        /// Skill name (if not provided, sets up all eligible skills)
        name: Option<String>,
    },
    /// Run a skill with the given input
    Run {
        /// Skill name/ID
        id: String,
        /// Input message for the skill
        #[arg(short, long)]
        input: String,
        /// Additional context (JSON)
        #[arg(short, long)]
        context: Option<String>,
    },
    /// Create a new skill template
    Init {
        /// Skill name
        name: String,
        /// Target directory (defaults to ./<name>-skill)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Template to use
        #[arg(short, long, default_value = "basic")]
        template: String,
    },
}

/// Run skill commands
pub async fn run_skill_command(command: &SkillCommands) -> Result<()> {
    let client = reqwest::Client::new();

    match command {
        SkillCommands::List { all, format } => {
            let mut url = format!("{}/api/v1/skills", DAEMON_URL);
            let mut params = Vec::new();
            if *all {
                params.push("all=true".to_string());
            }
            let fmt_str = match format {
                crate::cli::OutputFormat::Table => "table",
                crate::cli::OutputFormat::Json => "json",
                crate::cli::OutputFormat::Yaml => "yaml",
                crate::cli::OutputFormat::Plain => "plain",
            };
            params.push(format!("format={}", fmt_str));
            if !params.is_empty() {
                url.push('?');
                url.push_str(&params.join("&"));
            }
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon at {}: {}", DAEMON_URL, e);
                    eprintln!("Is the daemon running? Try: syscity start");
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
        SkillCommands::Info { name } => {
            let url = format!("{}/api/v1/skills/{}", DAEMON_URL, name);
            match client.get(&url).send().await {
                Ok(resp) => {
                    let body = resp.text().await.unwrap_or_default();
                    println!("{}", body);
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
        SkillCommands::Install { source, name, registry } => {
            if *registry {
                // Install from remote registry via daemon API
                let url = format!("{}/api/v1/skills/install", DAEMON_URL);
                let body = serde_json::json!({
                    "name": name.clone().unwrap_or_else(|| source.clone()),
                    "registry_url": null,
                });
                match client.post(&url).json(&body).send().await {
                    Ok(resp) => {
                        let status = resp.status();
                        let text = resp.text().await.unwrap_or_default();
                        if status.is_success() {
                            println!("Skill '{}' installed from registry", source);
                        } else {
                            eprintln!("Failed to install skill ({}): {}", status, text);
                        }
                    }
                    Err(e) => {
                        eprintln!("Failed to reach daemon: {}", e);
                        return Err(SyscityError::Internal(e.to_string()));
                    }
                }
            } else {
                install_skill_local(source, name.as_deref()).await?;
            }
        }
        SkillCommands::Uninstall { name, force } => {
            if !force {
                println!("Uninstall skill '{}'? Use --force to confirm.", name);
                return Ok(());
            }
            // Uninstall via daemon API
            let url = format!("{}/api/v1/skills/{}/uninstall", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Skill '{}' uninstalled", name);
                    } else {
                        eprintln!("Failed to uninstall skill ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
        SkillCommands::Enable { name } => {
            let url = format!("{}/api/v1/skills/{}/enable", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Skill '{}' enabled", name);
                    } else {
                        eprintln!("Failed to enable skill ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
        SkillCommands::Disable { name } => {
            let url = format!("{}/api/v1/skills/{}/disable", DAEMON_URL, name);
            match client.post(&url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Skill '{}' disabled", name);
                    } else {
                        eprintln!("Failed to disable skill ({}): {}", status, text);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
        SkillCommands::Setup { name } => {
            setup_skill_deps(name.as_deref()).await?;
        }
        SkillCommands::Init { name, path, template } => {
            init_skill_template(name, path.as_deref(), template).await?;
        }
        SkillCommands::Run { id, input, context } => {
            let url = format!("{}/api/v1/skills/{}/run", DAEMON_URL, id);
            let body = serde_json::json!({
                "input": input,
                "context": context.as_ref().and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok()),
            });
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    let body: serde_json::Value = resp.json().await.unwrap_or_default();
                    if status.is_success() {
                        println!("Skill '{}' executed successfully", id);
                        if let Some(result) = body.get("result").and_then(|r| r.as_str()) {
                            println!("\n{}", result);
                        }
                        if let Some(usage) = body.get("usage") {
                            println!("\nUsage: {}", usage);
                        }
                    } else {
                        eprintln!("Failed to run skill ({}): {}", status, body);
                    }
                }
                Err(e) => {
                    eprintln!("Failed to reach daemon: {}", e);
                    return Err(SyscityError::Internal(e.to_string()));
                }
            }
        }
    }
    Ok(())
}

/// Install a skill from a local path or git URL into the user skills directory.
async fn install_skill_local(source: &str, name: Option<&str>) -> Result<()> {
    let skills_dir = crate::dirs::skills_dir();

    // Determine skill name from source or explicit override
    let skill_name = if let Some(n) = name {
        n.to_string()
    } else {
        // Derive name from last path/URL component, strip .git suffix
        source
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(source)
            .trim_end_matches(".git")
            .to_string()
    };

    let dest = skills_dir.join(&skill_name);

    // Detect git URL vs local path
    if source.starts_with("http://") || source.starts_with("https://") || source.ends_with(".git") {
        println!("Cloning skill '{}' from {}", skill_name, source);
        let status = tokio::process::Command::new("git")
            .args([
                "clone",
                "--depth=1",
                source,
                dest.to_str().unwrap_or_default(),
            ])
            .status()
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to run git clone: {}", e)))?;
        if !status.success() {
            return Err(SyscityError::Internal(format!("git clone failed for '{}'", source)));
        }
    } else {
        // Local directory copy
        let src_path = std::path::Path::new(source);
        if !src_path.exists() {
            return Err(SyscityError::Internal(format!("Source path does not exist: {}", source)));
        }
        copy_dir_recursive(src_path, &dest).await.map_err(|e| {
            SyscityError::Internal(format!("Failed to copy skill directory: {}", e))
        })?;
    }

    println!("Skill '{}' installed to {:?}", skill_name, dest);
    println!("Run 'syscity skill setup {}' to install its dependencies.", skill_name);
    Ok(())
}

/// Recursively copy a directory tree.
async fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dst).await?;
    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&entry.path(), &dst_path)).await?;
        } else {
            tokio::fs::copy(entry.path(), &dst_path).await?;
        }
    }
    Ok(())
}

/// Run install specs for one skill (or all skills in the skills dir).
async fn setup_skill_deps(name: Option<&str>) -> Result<()> {
    let skills_dir = crate::dirs::skills_dir();

    let skill_dirs: Vec<PathBuf> = if let Some(n) = name {
        vec![skills_dir.join(n)]
    } else {
        // Collect all subdirectories of the skills dir
        let mut dirs = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&skills_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    dirs.push(entry.path());
                }
            }
        }
        dirs
    };

    for dir in skill_dirs {
        let skill_md = dir.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }
        let content = tokio::fs::read_to_string(&skill_md)
            .await
            .map_err(|e| SyscityError::Internal(format!("Failed to read {:?}: {}", skill_md, e)))?;

        let skill_file = match SkillFile::parse(&content, skill_md.clone()) {
            Ok(sf) => sf,
            Err(e) => {
                eprintln!("Skipping {:?}: failed to parse SKILL.md: {}", dir, e);
                continue;
            }
        };

        let specs = &skill_file.frontmatter.install;
        if specs.is_empty() {
            println!("No install specs for skill '{}'", skill_file.frontmatter.name);
            continue;
        }

        println!(
            "Installing {} dep(s) for skill '{}'...",
            specs.len(),
            skill_file.frontmatter.name
        );
        let results = install_all(specs).await;
        for (spec, result) in results {
            println!("  {:?} -> {:?}", spec, result);
        }
    }
    Ok(())
}

/// Create a new SKILL.md template in the given directory.
async fn init_skill_template(
    name: &str,
    path: Option<&std::path::Path>,
    template: &str,
) -> Result<()> {
    let target_dir = if let Some(p) = path {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_default()
            .join(format!("{}-skill", name))
    };

    tokio::fs::create_dir_all(&target_dir).await.map_err(|e| {
        SyscityError::Internal(format!("Failed to create directory {:?}: {}", target_dir, e))
    })?;

    let skill_md_content = match template {
        "basic" => format!(
            r#"---
name: {name}
version: "0.1.0"
description: "A brief description of {name}"
author: ""
triggers:
  - type: command
    pattern: "/{name}"
    user_invocable: true
install: []
requires:
  bins: []
---

# {name}

Describe what this skill does here.

## Usage

Describe how to use this skill.
"#,
            name = name
        ),
        _ => format!(
            r#"---
name: {name}
version: "0.1.0"
description: "A brief description of {name}"
author: ""
triggers: []
install: []
---

# {name}
"#,
            name = name
        ),
    };

    let skill_md_path = target_dir.join("SKILL.md");
    tokio::fs::write(&skill_md_path, skill_md_content)
        .await
        .map_err(|e| SyscityError::Internal(format!("Failed to write SKILL.md: {}", e)))?;

    println!("Created skill '{}' at {:?}", name, target_dir);
    println!("Edit {:?} to configure your skill.", skill_md_path);
    Ok(())
}
