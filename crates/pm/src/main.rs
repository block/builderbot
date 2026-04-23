mod cmd;
mod core;

use anyhow::{Result, anyhow};
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

        /// Use an existing local checkout instead of cloning (for heavy repos).
        /// Default: symlinks directly (pm won't manage branches).
        /// Combine with --worktree to get per-project branches without cloning.
        #[arg(long)]
        existing: Option<PathBuf>,

        /// With --existing: create worktrees from the existing repo so pm manages branches.
        /// Without this, --existing just symlinks the checkout directly.
        #[arg(long, requires = "existing")]
        worktree: bool,

        /// Evict a specific project to free a pool slot (non-interactive)
        #[arg(long)]
        evict: Option<String>,

        /// Grow the pool by one slot instead of evicting
        #[arg(long, conflicts_with = "evict")]
        grow_pool: bool,
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

    #[command(
        after_help = "Examples:\n  pm find dev/create-wallet-address\n  pm find origin/dev/create-wallet-address\n  pm find create-wallet-address\n  cd \"$(pm --root ~/projects find create-wallet-address)\""
    )]
    /// Find the project repo path for a branch in this workspace
    Find {
        /// Branch name copied from GitHub or git
        branch: String,
    },
}

fn styling() -> clap::builder::Styles {
    clap::builder::Styles::styled()
        .header(clap::builder::styling::AnsiColor::Green.on_default().bold())
        .usage(clap::builder::styling::AnsiColor::Green.on_default().bold())
        .literal(clap::builder::styling::AnsiColor::Cyan.on_default().bold())
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
    let Cli { root, command } = Cli::parse();
    let base = resolve_root(root)?;
    let workspace_exists = base.join(".pm").join("state.json").exists();

    // Auto-init if no workspace exists
    if matches!(&command, Commands::Find { .. }) && !workspace_exists {
        return Err(anyhow!(
            "No pm workspace found. Use --root <path> or run `pm new <project>` in the workspace you want to search."
        ));
    }

    if !matches!(&command, Commands::Find { .. }) && !workspace_exists {
        cmd::init(&base)?;
    }

    match command {
        Commands::New { name } => cmd::new(&base, &name),
        Commands::Add {
            repo,
            branch,
            project,
            existing,
            worktree,
            evict,
            grow_pool,
        } => {
            let project_name = project.or_else(|| infer_project(&base)).ok_or_else(|| {
                anyhow::anyhow!(
                    "Can't infer project from cwd. Use --project or cd into a project dir."
                )
            })?;
            cmd::add(
                &base,
                &project_name,
                &repo,
                branch.as_deref(),
                existing,
                worktree,
                cmd::AddConflictOpts { evict, grow_pool },
            )
        }
        Commands::Rm { name } => cmd::rm(&base, &name),
        Commands::Status => cmd::status(&base),
        Commands::Cleanup { stale_days } => cmd::cleanup(&base, stale_days),
        Commands::Find { branch } => cmd::find(&base, &branch),
    }
}
