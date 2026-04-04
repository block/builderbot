use anyhow::{Context, Result, bail};
use chrono::Utc;
use colored::Colorize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::core::git;
use crate::core::pool;
use crate::core::state::{self, RepoEntry, State};

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

    let (repo_name, url, needs_clone) = resolve_repo(&state, repo_input);

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

    let slot_idx = pool::acquire_slot(&mut state, &repo_name, project, &branch)?;
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

        // Check if branches are merged
        for (repo_name, branch) in &project.repos {
            if let Some(repo) = state.repos.get(repo_name) {
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
