mod cmd;
mod core;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "pm",
    about = "Project manager for multi-repo workspaces",
    version,
    styles = styling(),
)]
struct Cli {
    /// Root directory for pm workspace (default: auto-detected)
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new project
    New {
        /// Project name
        name: String,
    },

    /// Add a repo to the current project
    Add {
        /// Repo: owner/name shorthand, full URL, or pool repo name
        repo: String,

        /// Explicit branch (default: $USER/$PROJECT)
        #[arg(long)]
        branch: Option<String>,

        /// Target project (default: inferred from cwd)
        #[arg(long)]
        project: Option<String>,
    },

    /// Remove a project (also cleans up stale state from manual rm)
    Rm {
        /// Project name
        name: String,
    },

    /// Show status of projects and repos
    Status,

    /// Analyze projects and recommend cleanup
    Cleanup {
        /// Days of inactivity before a project is considered stale
        #[arg(long, default_value = "14")]
        stale_days: u64,
    },
}

fn styling() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .header(
            clap::builder::styling::AnsiColor::Green
                .on_default()
                .bold(),
        )
        .usage(
            clap::builder::styling::AnsiColor::Green
                .on_default()
                .bold(),
        )
        .literal(
            clap::builder::styling::AnsiColor::Cyan
                .on_default()
                .bold(),
        )
        .placeholder(clap::builder::styling::AnsiColor::Cyan.on_default())
}

/// Walk up from cwd looking for .pm/state.json
fn resolve_root(cli_root: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(root) = cli_root {
        return Ok(root);
    }

    let cwd = std::env::current_dir()?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join(".pm").join("state.json").exists() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }

    Ok(cwd)
}

/// Infer which project we're in from cwd relative to workspace root
fn infer_project(base: &std::path::Path) -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let relative = cwd.strip_prefix(base).ok()?;
    let first = relative.components().next()?;
    let name = first.as_os_str().to_string_lossy().to_string();
    // Don't match .pm
    if name.starts_with('.') {
        return None;
    }
    Some(name)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let base = resolve_root(cli.root)?;

    // Auto-init if no workspace exists
    if !base.join(".pm").join("state.json").exists() {
        cmd::init(&base)?;
    }

    match cli.command {
        Commands::New { name } => cmd::new(&base, &name),
        Commands::Add {
            repo,
            branch,
            project,
        } => {
            let project_name = project
                .or_else(|| infer_project(&base))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Can't infer project from cwd. Use --project or cd into a project dir."
                    )
                })?;
            cmd::add(&base, &project_name, &repo, branch.as_deref())
        }
        Commands::Rm { name } => cmd::rm(&base, &name),
        Commands::Status => cmd::status(&base),
        Commands::Cleanup { stale_days } => cmd::cleanup(&base, stale_days),
    }
}
