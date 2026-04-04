use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Command, Stdio};

/// Try to clone bare. Returns Ok(()) on success, Err on failure.
/// Suppresses git output — we show our own messages.
fn try_clone_bare(url: &str, dest: &Path) -> Result<()> {
    // Clean up dest if a previous attempt left it around
    if dest.exists() {
        let _ = std::fs::remove_dir_all(dest);
    }

    let output = Command::new("git")
        .args(["clone", "--bare", url])
        .arg(dest)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to run git")?;

    if !output.status.success() {
        // Clean up partial clone
        if dest.exists() {
            let _ = std::fs::remove_dir_all(dest);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.trim());
    }
    Ok(())
}

/// Convert between HTTPS and SSH URLs for GitHub
fn alternate_url(url: &str) -> Option<String> {
    // SSH → HTTPS
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return Some(format!("https://github.com/{}", rest));
    }
    // HTTPS → SSH
    if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        return Some(format!("git@github.com:{}", rest));
    }
    None
}

/// Clone a bare repo, trying HTTPS first then SSH (or vice versa).
/// Returns the URL that actually worked.
pub fn clone_bare(url: &str, dest: &Path) -> Result<String> {
    // Try primary URL
    match try_clone_bare(url, dest) {
        Ok(()) => {
            configure_bare(dest);
            Ok(url.to_string())
        }
        Err(_primary_err) => {
            // Try alternate protocol
            if let Some(alt) = alternate_url(url)
                && let Ok(()) = try_clone_bare(&alt, dest)
            {
                configure_bare(dest);
                return Ok(alt);
            }
            // Both failed — give a clean error
            bail!(
                "Could not clone {}. Check the URL and your access permissions.",
                url
            );
        }
    }
}

/// Configure a bare clone for proper fetching
fn configure_bare(dest: &Path) {
    let _ = Command::new("git")
        .args([
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ])
        .current_dir(dest)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let _ = Command::new("git")
        .args(["fetch", "--all"])
        .current_dir(dest)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Create a worktree from a bare repo
pub fn add_worktree(bare_path: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    let _ = ensure_fetched(bare_path);

    // Try checking out existing branch
    let output = Command::new("git")
        .args(["worktree", "add", "--force"])
        .arg(worktree_path)
        .arg(branch)
        .current_dir(bare_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to run git worktree add")?;

    if output.status.success() {
        return Ok(());
    }

    // Branch might not exist — try creating from default branch
    let default = default_branch(bare_path).unwrap_or_else(|_| "main".to_string());
    let output = Command::new("git")
        .args(["worktree", "add", "--force", "-b", branch])
        .arg(worktree_path)
        .arg(format!("origin/{}", default))
        .current_dir(bare_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to run git worktree add")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "Failed to create worktree for branch '{}': {}",
        branch,
        stderr.trim()
    );
}

/// Remove a worktree
#[allow(dead_code)]
pub fn remove_worktree(bare_path: &Path, worktree_path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(worktree_path)
        .current_dir(bare_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("Failed to run git worktree remove")?;
    if !output.status.success() {
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(bare_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    Ok(())
}

/// Checkout a branch in an existing worktree directory
pub fn checkout(worktree_path: &Path, branch: &str) -> Result<()> {
    let _ = Command::new("git")
        .args(["fetch", "--all", "--prune"])
        .current_dir(worktree_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let output = Command::new("git")
        .args(["checkout", branch])
        .current_dir(worktree_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to run git checkout")?;

    if !output.status.success() {
        let output = Command::new("git")
            .args(["checkout", "-b", branch, &format!("origin/{}", branch)])
            .current_dir(worktree_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .context("Failed to create tracking branch")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to checkout '{}': {}", branch, stderr.trim());
        }
    }
    Ok(())
}

/// Get the default branch of a bare repo (usually main or master)
pub fn default_branch(bare_path: &Path) -> Result<String> {
    let _ = ensure_fetched(bare_path);

    let output = Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(bare_path)
        .stderr(Stdio::null())
        .output()
        .context("Failed to get default branch")?;

    if output.status.success() {
        let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // refs/remotes/origin/main -> main
        if let Some(branch) = refname.strip_prefix("refs/remotes/origin/") {
            return Ok(branch.to_string());
        }
    }

    // Fallback: try main, then master
    for candidate in &["main", "master"] {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", &format!("origin/{}", candidate)])
            .current_dir(bare_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(o) = output
            && o.status.success()
        {
            return Ok(candidate.to_string());
        }
    }

    Ok("main".to_string())
}

/// Check if a branch has been merged into the default branch
pub fn is_branch_merged(bare_path: &Path, branch: &str, default: &str) -> Result<bool> {
    let _ = ensure_fetched(bare_path);

    let output = Command::new("git")
        .args(["branch", "-r", "--merged", &format!("origin/{}", default)])
        .current_dir(bare_path)
        .stderr(Stdio::null())
        .output()
        .context("Failed to check merged branches")?;

    if !output.status.success() {
        return Ok(false);
    }

    let merged = String::from_utf8_lossy(&output.stdout);
    let target = format!("origin/{}", branch);
    Ok(merged.lines().any(|line| line.trim() == target))
}

/// Get the last commit date on a branch
#[allow(dead_code)]
pub fn last_commit_date(bare_path: &Path, branch: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ci", &format!("origin/{}", branch)])
        .current_dir(bare_path)
        .stderr(Stdio::null())
        .output()
        .context("Failed to get last commit date")?;

    if output.status.success() {
        let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !date.is_empty() {
            return Ok(Some(date));
        }
    }
    Ok(None)
}

/// Ensure a bare repo has the fetch refspec configured, then fetch
fn ensure_fetched(bare_path: &Path) -> Result<()> {
    let _ = Command::new("git")
        .args([
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ])
        .current_dir(bare_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let output = Command::new("git")
        .args(["fetch", "--all", "--prune"])
        .current_dir(bare_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .context("Failed to fetch")?;
    if !output.status.success() {
        bail!("git fetch failed");
    }
    Ok(())
}
