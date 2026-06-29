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

        /// Copy AGENTS.md into the project and symlink CLAUDE.md to it.
        /// When passed without a path, uses AGENTS.md from the current directory.
        #[arg(long, value_name = "PATH", num_args = 0..=1, require_equals = true)]
        agents_md: Option<Option<PathBuf>>,
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
    let cli = Cli::parse();
    let base = resolve_root(cli.root)?;

    // Auto-init if no workspace exists
    if !base.join(".pm").join("state.json").exists() {
        cmd::init(&base)?;
    }

    match cli.command {
        Commands::New { name, agents_md } => {
            let agents_md =
                agents_md.map(|path| path.unwrap_or_else(|| PathBuf::from("AGENTS.md")));
            cmd::new(&base, &name, agents_md.as_deref())
        }
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
    }
}
