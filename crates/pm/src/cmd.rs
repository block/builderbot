use anyhow::{Context, Result, bail};
use chrono::Utc;
use colored::Colorize;
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::core::git;
use crate::core::pool::{self, AcquireResult, EvictionNeeded};
use crate::core::state::{self, RepoEntry, State};

/// Options for resolving pool conflicts during `add`
#[derive(Debug, Default)]
pub struct AddConflictOpts {
    /// Evict a specific project to free a pool slot (non-interactive)
    pub evict: Option<String>,
    /// Grow the pool by one slot instead of evicting
    pub grow_pool: bool,
}

// ── init ────────────────────────────────────────────────────────────────

pub fn init(base: &Path) -> Result<()> {
    std::fs::create_dir_all(State::repos_dir(base)).context("Failed to create repos directory")?;
    std::fs::create_dir_all(State::pool_dir(base)).context("Failed to create pool directory")?;

    let state = State::new(base.to_path_buf());
    state.save()?;

    println!(
        "{} Initialized pm workspace at {}",
        "✓".green().bold(),
        base.display()
    );
    Ok(())
}

// ── new ─────────────────────────────────────────────────────────────────

pub fn new(base: &Path, name: &str) -> Result<()> {
    let mut state = State::load_or_err(base)?;

    // Handle orphaned state: project in state but dir is gone
    if state.projects.contains_key(name) {
        let project_dir = base.join(name);
        if !project_dir.exists() {
            println!(
                "{} Cleaning up stale state for {}...",
                "→".yellow().bold(),
                name.bold()
            );
            pool::release_project(&mut state, name);
            state.projects.remove(name);
            state.save()?;
        } else {
            bail!("Project '{}' already exists.", name);
        }
    }

    // Create project directory
    let project_dir = base.join(name);
    std::fs::create_dir_all(&project_dir)
        .with_context(|| format!("Failed to create {}", project_dir.display()))?;

    state.projects.insert(
        name.to_string(),
        state::Project {
            name: name.to_string(),
            repos: BTreeMap::new(),
            pinned: false,
            created_at: Utc::now(),
            last_activated: None,
        },
    );
    state.save()?;

    println!("{} Created project {}", "✓".green().bold(), name.bold());
    println!("  {}", project_dir.display().to_string().dimmed());
    println!(
        "\n  cd into it and run {} to add repos.",
        "pm add <repo>".cyan()
    );
    Ok(())
}

// ── add ─────────────────────────────────────────────────────────────────

/// Resolve a repo spec into a git URL.
///
/// Accepts:
///   - Full SSH URL:   git@github.com:org/repo.git
///   - Full HTTPS URL: https://github.com/org/repo
///   - Shorthand:      org/repo  →  git@github.com:org/repo.git
///   - Pool name:      repo      →  already cloned, reuse
fn resolve_repo(state: &State, input: &str) -> (String, String, bool) {
    // Already in pool by name?
    if state.repos.contains_key(input) {
        let entry = &state.repos[input];
        return (entry.name.clone(), entry.url.clone(), false);
    }

    // Full URL?
    if input.starts_with("git@") || input.starts_with("https://") || input.starts_with("http://") {
        let name = state::repo_name_from_url(input);
        let already = state.repos.contains_key(&name);
        return (name, input.to_string(), !already);
    }

    // Shorthand: org/repo → https://github.com/org/repo.git
    if input.contains('/') && !input.contains(':') && !input.contains("//") {
        let url = format!("https://github.com/{}.git", input);
        let name = state::repo_name_from_url(&url);
        let already = state.repos.contains_key(&name);
        return (name, url, !already);
    }

    // Last resort: treat as pool name that doesn't exist yet
    (input.to_string(), input.to_string(), false)
}

pub fn add(
    base: &Path,
    project: &str,
    repo_input: &str,
    branch_override: Option<&str>,
    existing: Option<PathBuf>,
    worktree: bool,
    conflict_opts: AddConflictOpts,
) -> Result<()> {
    let mut state = State::load_or_err(base)?;

    // Ensure project exists (handle orphaned state)
    if !state.projects.contains_key(project) {
        // Maybe the dir exists but state doesn't — create it
        let project_dir = base.join(project);
        if project_dir.exists() {
            state.projects.insert(
                project.to_string(),
                state::Project {
                    name: project.to_string(),
                    repos: BTreeMap::new(),
                    pinned: false,
                    created_at: Utc::now(),
                    last_activated: None,
                },
            );
            state.save()?;
            println!(
                "{} Registered existing directory as project {}",
                "→".yellow().bold(),
                project.bold()
            );
        } else {
            bail!(
                "Project '{}' not found. Run {} first.",
                project,
                format!("pm new {}", project).cyan()
            );
        }
    }

    // Handle --existing: either symlink directly or register for worktree use
    if let Some(ref ext_path) = existing {
        if worktree {
            return add_existing_worktree(
                base,
                &mut state,
                project,
                repo_input,
                branch_override,
                ext_path,
                &conflict_opts,
            );
        } else {
            return add_existing(
                base,
                &mut state,
                project,
                repo_input,
                branch_override,
                ext_path,
            );
        }
    }

    let (repo_name, url, needs_clone) = resolve_repo(&state, repo_input);

    // If this is an external repo, re-link it instead of doing worktree ops
    if let Some(repo) = state.repos.get(&repo_name)
        && repo.external
    {
        let ext_path = repo
            .external_path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("External repo '{}' has no path recorded", repo_name))?;
        return add_existing(
            base,
            &mut state,
            project,
            &repo_name,
            branch_override,
            &ext_path,
        );
    }

    // Clone into pool if needed
    if needs_clone {
        let bare_path = State::repos_dir(base).join(format!("{}.git", repo_name));
        println!("{} Cloning {}...", "→".blue().bold(), repo_name.bold());
        let actual_url = git::clone_bare(&url, &bare_path)?;

        state.repos.insert(
            repo_name.clone(),
            RepoEntry {
                url: actual_url,
                name: repo_name.clone(),
                bare_path,
                max_slots: pool::default_max_slots(),
                external: false,
                external_path: None,
            },
        );
        state.save()?;
    } else if !state.repos.contains_key(&repo_name) {
        bail!(
            "Repo '{}' not found in pool and doesn't look like a URL.\n\
             Try: pm add org/repo  or  pm add https://github.com/org/repo",
            repo_input
        );
    }

    // Determine branch
    let branch = match branch_override {
        Some(b) => b.to_string(),
        None => {
            let user = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "unknown".to_string());
            format!("{}/{}", user, project)
        }
    };

    // Add repo to project state
    state
        .projects
        .get_mut(project)
        .unwrap()
        .repos
        .insert(repo_name.clone(), branch.clone());

    // Acquire pool slot → worktree
    print!("  {} {}:{} ", "→".dimmed(), repo_name, branch.dimmed());

    let slot_idx = match pool::acquire_slot(&mut state, &repo_name, project, &branch)? {
        AcquireResult::Acquired(idx) => idx,
        AcquireResult::NeedsEviction(eviction) => {
            println!(); // finish the pending print line
            resolve_eviction(&mut state, eviction, project, &branch, &conflict_opts)?
        }
    };

    let slot_path = state.pool.slots[&repo_name][slot_idx].path.clone();

    // Symlink into project dir
    let project_dir = base.join(project);
    std::fs::create_dir_all(&project_dir)?;

    let link_path = project_dir.join(&repo_name);
    if link_path.is_symlink() || link_path.exists() {
        let _ = std::fs::remove_file(&link_path);
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(&slot_path, &link_path)
        .with_context(|| format!("symlink {} → {}", link_path.display(), slot_path.display()))?;

    #[cfg(not(unix))]
    std::os::windows::fs::symlink_dir(&slot_path, &link_path)
        .with_context(|| format!("symlink {} → {}", link_path.display(), slot_path.display()))?;

    println!("{}", "✓".green());

    // Update last_activated
    state.projects.get_mut(project).unwrap().last_activated = Some(Utc::now());
    state.save()?;

    println!(
        "\n{} {} ready at {}",
        "✓".green().bold(),
        repo_name.bold(),
        link_path.display().to_string().cyan()
    );
    Ok(())
}

/// Handle --existing: symlink an external checkout directly (no pool slot)
fn add_existing(
    base: &Path,
    state: &mut State,
    project: &str,
    repo_input: &str,
    _branch_override: Option<&str>,
    ext_path: &Path,
) -> Result<()> {
    // Resolve to absolute/canonical path
    let canonical = if ext_path.is_absolute() {
        ext_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(ext_path)
    };
    let canonical = canonical.canonicalize().with_context(|| {
        format!(
            "Path '{}' does not exist or is not accessible",
            ext_path.display()
        )
    })?;

    // Validate it's a git repo
    if !canonical.join(".git").exists() {
        bail!(
            "{} is not a git repository (no .git found).",
            canonical.display()
        );
    }

    // Derive repo name: use the input directly if it's a simple name,
    // otherwise extract from URL/path
    let repo_name =
        if repo_input.contains('/') || repo_input.contains(':') || repo_input.contains("//") {
            state::repo_name_from_url(repo_input)
        } else {
            repo_input.to_string()
        };

    // Get the remote URL for display/state
    let url = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&canonical)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| format!("local:{}", canonical.display()));

    // Get current branch for display
    let current_branch = std::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(&canonical)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "(detached)".to_string());

    // Register repo in state if not already there
    if !state.repos.contains_key(&repo_name) {
        println!(
            "{} Registering {} as external repo (no clone needed)",
            "→".blue().bold(),
            repo_name.bold()
        );
        state.repos.insert(
            repo_name.clone(),
            RepoEntry {
                url,
                name: repo_name.clone(),
                bare_path: PathBuf::new(), // unused for external
                max_slots: 1,              // no pool slots needed
                external: true,
                external_path: Some(canonical.clone()),
            },
        );
    } else {
        let entry = state.repos.get(&repo_name).unwrap();
        if !entry.external {
            bail!(
                "Repo '{}' already exists as a pooled (non-external) repo.\n\
                 Use a different name: pm add <other-name> --existing {}",
                repo_name,
                ext_path.display()
            );
        }
    }

    // Track as "(external)" since pm doesn't manage the branch
    state
        .projects
        .get_mut(project)
        .unwrap()
        .repos
        .insert(repo_name.clone(), "(external)".to_string());

    // Symlink the external path into the project dir
    let project_dir = base.join(project);
    std::fs::create_dir_all(&project_dir)?;

    let link_path = project_dir.join(&repo_name);
    if link_path.is_symlink() || link_path.exists() {
        let _ = std::fs::remove_file(&link_path);
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(&canonical, &link_path)
        .with_context(|| format!("symlink {} → {}", link_path.display(), canonical.display()))?;

    #[cfg(not(unix))]
    std::os::windows::fs::symlink_dir(&canonical, &link_path)
        .with_context(|| format!("symlink {} → {}", link_path.display(), canonical.display()))?;

    state.projects.get_mut(project).unwrap().last_activated = Some(Utc::now());
    state.save()?;

    println!(
        "{} {} linked as external repo (currently on {})",
        "✓".green().bold(),
        repo_name.bold(),
        current_branch.cyan()
    );
    println!(
        "  {} → {}",
        link_path.display().to_string().dimmed(),
        canonical.display().to_string().cyan()
    );
    println!(
        "\n  {} Branch management is yours — pm won't checkout or switch branches.",
        "ℹ".blue()
    );
    Ok(())
}

// ── add --existing --worktree ────────────────────────────────────────────

/// Register an existing checkout as a pool repo (skip clone, use worktrees).
///
/// Instead of bare-cloning, we point `bare_path` at the existing repo root.
/// `git worktree add` works from non-bare repos — it uses the same object store.
/// This gives you per-project branches without the multi-minute clone.
fn add_existing_worktree(
    base: &Path,
    state: &mut State,
    project: &str,
    repo_input: &str,
    branch_override: Option<&str>,
    ext_path: &Path,
    conflict_opts: &AddConflictOpts,
) -> Result<()> {
    // Resolve to absolute/canonical path
    let canonical = if ext_path.is_absolute() {
        ext_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(ext_path)
    };
    let canonical = canonical.canonicalize().with_context(|| {
        format!(
            "Path '{}' does not exist or is not accessible",
            ext_path.display()
        )
    })?;

    // Validate it's a git repo
    if !canonical.join(".git").exists() {
        bail!(
            "{} is not a git repository (no .git found).",
            canonical.display()
        );
    }

    // Derive repo name
    let repo_name =
        if repo_input.contains('/') || repo_input.contains(':') || repo_input.contains("//") {
            state::repo_name_from_url(repo_input)
        } else {
            repo_input.to_string()
        };

    // Get the remote URL for state
    let url = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&canonical)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| format!("local:{}", canonical.display()));

    // Register repo in state — bare_path points to the existing repo root
    // (git worktree add works from non-bare repos just fine)
    if !state.repos.contains_key(&repo_name) {
        println!(
            "{} Registering {} from existing checkout (worktree mode, no clone)",
            "→".blue().bold(),
            repo_name.bold()
        );
        state.repos.insert(
            repo_name.clone(),
            RepoEntry {
                url,
                name: repo_name.clone(),
                bare_path: canonical.clone(), // the existing repo root
                max_slots: pool::default_max_slots(),
                external: false,
                external_path: Some(canonical.clone()),
            },
        );
        state.save()?;
    } else {
        let entry = state.repos.get(&repo_name).unwrap();
        if entry.external {
            bail!(
                "Repo '{}' is already registered as external (symlink-only).\n\
                 Remove it first with `pm rm` on projects using it, then re-add with --worktree.",
                repo_name,
            );
        }
    }

    // Determine branch
    let branch = match branch_override {
        Some(b) => b.to_string(),
        None => {
            let user = std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "unknown".to_string());
            format!("{}/{}", user, project)
        }
    };

    // Add repo to project state
    state
        .projects
        .get_mut(project)
        .unwrap()
        .repos
        .insert(repo_name.clone(), branch.clone());

    // Acquire pool slot → worktree (normal pool machinery)
    print!("  {} {}:{} ", "→".dimmed(), repo_name, branch.dimmed());

    let slot_idx = match pool::acquire_slot(state, &repo_name, project, &branch)? {
        AcquireResult::Acquired(idx) => idx,
        AcquireResult::NeedsEviction(eviction) => {
            println!();
            resolve_eviction(state, eviction, project, &branch, conflict_opts)?
        }
    };

    let slot_path = state.pool.slots[&repo_name][slot_idx].path.clone();

    // Symlink into project dir
    let project_dir = base.join(project);
    std::fs::create_dir_all(&project_dir)?;

    let link_path = project_dir.join(&repo_name);
    if link_path.is_symlink() || link_path.exists() {
        let _ = std::fs::remove_file(&link_path);
    }

    #[cfg(unix)]
    std::os::unix::fs::symlink(&slot_path, &link_path)
        .with_context(|| format!("symlink {} → {}", link_path.display(), slot_path.display()))?;

    #[cfg(not(unix))]
    std::os::windows::fs::symlink_dir(&slot_path, &link_path)
        .with_context(|| format!("symlink {} → {}", link_path.display(), slot_path.display()))?;

    println!("{}", "✓".green());

    state.projects.get_mut(project).unwrap().last_activated = Some(Utc::now());
    state.save()?;

    println!(
        "\n{} {} ready at {} (worktree from {})",
        "✓".green().bold(),
        repo_name.bold(),
        link_path.display().to_string().cyan(),
        canonical.display().to_string().dimmed(),
    );
    Ok(())
}

// ── eviction resolution ─────────────────────────────────────────────────

/// Resolve a pool conflict — interactive menu, --evict, --grow-pool, or abort.
fn resolve_eviction(
    state: &mut State,
    eviction: EvictionNeeded,
    project_name: &str,
    branch: &str,
    opts: &AddConflictOpts,
) -> Result<usize> {
    let repo_name = &eviction.repo_name;

    // --grow-pool flag: just grow and go
    if opts.grow_pool {
        return pool::grow_and_acquire(state, repo_name, project_name, branch);
    }

    // --evict flag: find the named project and evict it
    if let Some(ref evict_target) = opts.evict {
        let candidate = eviction
            .candidates
            .iter()
            .find(|c| c.owner == *evict_target)
            .ok_or_else(|| {
                let available: Vec<&str> = eviction
                    .candidates
                    .iter()
                    .map(|c| c.owner.as_str())
                    .collect();
                anyhow::anyhow!(
                    "Project '{}' is not using a slot for '{}'. Evictable projects: {}",
                    evict_target,
                    repo_name,
                    available.join(", ")
                )
            })?;

        eprintln!(
            "  {} Evicting {} (branch: {}) from {} slot.",
            "⚠".yellow().bold(),
            candidate.owner.bold(),
            candidate.branch.as_deref().unwrap_or("?").dimmed(),
            repo_name.bold(),
        );

        return pool::execute_eviction(
            state,
            repo_name,
            candidate.slot_index,
            project_name,
            branch,
        );
    }

    // Non-interactive (agent/CI): abort with actionable guidance
    if !std::io::stdin().is_terminal() {
        let mut msg = format!(
            "All {} pool slots for '{}' are in use. Cannot evict interactively.\n\n\
             Slots in use:\n",
            eviction.current_max_slots, repo_name,
        );
        for c in &eviction.candidates {
            let age = format_age(c.last_used);
            msg.push_str(&format!(
                "  • {} (branch: {}, last used {})\n",
                c.owner,
                c.branch.as_deref().unwrap_or("?"),
                age,
            ));
        }
        msg.push_str(&format!(
            "\nTo resolve, re-run with one of:\n\
             \x20 pm add {} --evict <project>    evict a specific project\n\
             \x20 pm add {} --grow-pool           add another slot\n",
            repo_name, repo_name,
        ));
        bail!("{}", msg);
    }

    // Interactive: show a menu
    interactive_eviction_menu(state, eviction, project_name, branch)
}

/// Interactive menu for choosing how to resolve a pool conflict.
fn interactive_eviction_menu(
    state: &mut State,
    eviction: EvictionNeeded,
    project_name: &str,
    branch: &str,
) -> Result<usize> {
    use dialoguer::{Select, theme::ColorfulTheme};

    let repo_name = &eviction.repo_name;

    eprintln!();
    eprintln!(
        "  {} All {} slots for {} are in use.",
        "⚠".yellow().bold(),
        eviction.current_max_slots,
        repo_name.bold(),
    );
    eprintln!();

    // Build menu items
    let mut items: Vec<String> = Vec::new();

    // Sort candidates by last_used (oldest first = most likely eviction target)
    let mut candidates = eviction.candidates.clone();
    candidates.sort_by_key(|c| c.last_used);

    for c in &candidates {
        let age = format_age(c.last_used);
        items.push(format!(
            "Evict {} (branch: {}, last used {})",
            c.owner,
            c.branch.as_deref().unwrap_or("?"),
            age,
        ));
    }

    items.push(format!(
        "Grow pool to {} slots",
        eviction.current_max_slots + 1
    ));
    items.push("Abort".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("What would you like to do?")
        .items(&items)
        .default(0)
        .interact()?;

    let evict_count = candidates.len();

    if selection < evict_count {
        // Evict selected project
        let candidate = &candidates[selection];
        eprintln!("  {} Evicting {}...", "→".dimmed(), candidate.owner.bold(),);
        pool::execute_eviction(state, repo_name, candidate.slot_index, project_name, branch)
    } else if selection == evict_count {
        // Grow pool
        pool::grow_and_acquire(state, repo_name, project_name, branch)
    } else {
        // Abort
        bail!("Aborted.");
    }
}

/// Format a timestamp as a human-readable relative age
fn format_age(dt: chrono::DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(dt);

    if duration.num_days() > 0 {
        format!("{}d ago", duration.num_days())
    } else if duration.num_hours() > 0 {
        format!("{}h ago", duration.num_hours())
    } else {
        format!("{}m ago", duration.num_minutes().max(1))
    }
}

// ── rm ──────────────────────────────────────────────────────────────────

pub fn rm(base: &Path, name: &str) -> Result<()> {
    let mut state = State::load_or_err(base)?;

    let project_dir = base.join(name);

    // Clean up worktree symlinks + pool slots if project is in state
    if let Some(project) = state.projects.get(name).cloned() {
        for repo_name in project.repos.keys() {
            let link = project_dir.join(repo_name);
            if link.is_symlink() || link.exists() {
                let _ = std::fs::remove_file(&link);
            }
        }
        pool::release_project(&mut state, name);
    }

    // Remove project directory if it still exists
    if project_dir.exists() {
        std::fs::remove_dir_all(&project_dir)
            .with_context(|| format!("Failed to remove {}", project_dir.display()))?;
    }

    // Remove from state
    state.projects.remove(name);
    state.save()?;

    println!("{} Removed project {}", "✓".green().bold(), name.bold());
    Ok(())
}

// ── status ──────────────────────────────────────────────────────────────

pub fn status(base: &Path) -> Result<()> {
    let mut state = State::load_or_err(base)?;

    // First pass: detect and clean orphaned projects
    let orphaned: Vec<String> = state
        .projects
        .keys()
        .filter(|name| !base.join(name).exists())
        .cloned()
        .collect();

    for name in &orphaned {
        pool::release_project(&mut state, name);
        state.projects.remove(name);
        println!(
            "{} Cleaned up orphaned project {}",
            "→".yellow().bold(),
            name.bold()
        );
    }
    if !orphaned.is_empty() {
        state.save()?;
        println!();
    }

    // Projects
    println!("{}", "Projects".bold().underline());
    if state.projects.is_empty() {
        println!("  {}", "(none)".dimmed());
    }
    for (name, project) in &state.projects {
        let repo_count = project.repos.len();
        let active = project
            .last_activated
            .map(|t| format!("active {}", t.format("%Y-%m-%d")))
            .unwrap_or_else(|| "never used".to_string());

        println!(
            "  {} {} — {} repo(s), {}",
            "•".blue(),
            name.bold(),
            repo_count,
            active.dimmed()
        );

        for (repo, branch) in &project.repos {
            let slot_info = state
                .pool
                .slots
                .get(repo)
                .and_then(|slots| {
                    slots
                        .iter()
                        .find(|s| s.owner.as_deref() == Some(name.as_str()))
                })
                .map(|s| {
                    if s.path.exists() {
                        "✓".green().to_string()
                    } else {
                        "✗".red().to_string()
                    }
                })
                .unwrap_or_else(|| "–".dimmed().to_string());

            println!(
                "    {} {} {}:{}",
                slot_info,
                "→".dimmed(),
                repo,
                branch.dimmed()
            );
        }
    }

    println!();

    // Pool repos
    println!("{}", "Pool".bold().underline());
    if state.repos.is_empty() {
        println!("  {}", "(none)".dimmed());
    }
    for (name, repo) in &state.repos {
        if repo.external {
            let ext_path = repo
                .external_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "?".to_string());
            println!(
                "  {} {} {}  {}",
                "•".blue(),
                name.bold(),
                "(external)".yellow(),
                ext_path.dimmed()
            );
        } else {
            let slots = state.pool.slots.get(name);
            let (used, total) = slots
                .map(|s| (s.iter().filter(|sl| sl.owner.is_some()).count(), s.len()))
                .unwrap_or((0, repo.max_slots));
            println!(
                "  {} {} {}/{} slots  {}",
                "•".blue(),
                name.bold(),
                used,
                total,
                repo.url.dimmed()
            );
        }
    }

    Ok(())
}

// ── cleanup ─────────────────────────────────────────────────────────────

pub fn cleanup(base: &Path, stale_days: u64) -> Result<()> {
    let state = State::load_or_err(base)?;

    if state.projects.is_empty() {
        println!("{}", "No projects to analyze.".dimmed());
        return Ok(());
    }

    println!(
        "{} Analyzing {} project(s)...\n",
        "🔍".bold(),
        state.projects.len()
    );

    let now = Utc::now();
    let mut found = 0;

    for (name, project) in &state.projects {
        let mut reasons: Vec<String> = Vec::new();

        // Check if directory is missing
        if !base.join(name).exists() {
            reasons.push("directory missing (manually removed?)".to_string());
        }

        // Check staleness
        if let Some(last) = project.last_activated {
            let days = (now - last).num_days();
            if days > stale_days as i64 {
                reasons.push(format!("inactive for {} days", days));
            }
        } else {
            reasons.push("never used".to_string());
        }

        // Check if branches are merged (skip external repos — no bare clone to inspect)
        for (repo_name, branch) in &project.repos {
            if let Some(repo) = state.repos.get(repo_name) {
                if repo.external {
                    continue;
                }
                let default = git::default_branch(&repo.bare_path).unwrap_or("main".to_string());
                if branch != &default
                    && let Ok(true) = git::is_branch_merged(&repo.bare_path, branch, &default)
                {
                    reasons.push(format!("{}:{} merged into {}", repo_name, branch, default));
                }
            }
        }

        if !reasons.is_empty() {
            found += 1;
            println!("  {} {}", "•".yellow(), name.bold());
            for reason in &reasons {
                println!("    {} {}", "→".dimmed(), reason.dimmed());
            }
            println!(
                "    {} {}",
                "fix:".dimmed(),
                format!("pm rm {}", name).cyan()
            );
            println!();
        }
    }

    if found == 0 {
        println!("{} All projects look active.", "✓".green().bold());
    } else {
        println!("📋 {} project(s) may be ready for cleanup.", found);
    }

    Ok(())
}
