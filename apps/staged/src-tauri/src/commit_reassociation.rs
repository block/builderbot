//! Reattach commit metadata to rewritten SHAs after a rebase.
//!
//! Staged links each commit to the session that authored it by SHA. A rebase
//! gives every commit on the branch a new SHA, which orphans every one of
//! those rows: the timeline's SHA lookup misses, the commits lose their
//! session, and their reviews get hidden by `review_is_visible_in_timeline`.
//!
//! The mapping is recoverable from the DB plus git alone — no pre-rebase
//! capture, so this also survives an app restart mid-pipeline. `git rebase`
//! preserves author email, author date, and subject; only the SHA and the
//! committer fields change (conflict resolution doesn't touch author metadata,
//! and `--signoff` only appends a body trailer). The orphaned rows still hold
//! the old SHAs, and the old commit objects stay resolvable in the repo, so we
//! can read the old metadata back and match it against the rewritten commits.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use crate::store::Store;

/// The commit metadata `git rebase` carries across a rewrite. Two commits with
/// the same identity are the same commit before and after a rebase.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitIdentity {
    pub author_email: String,
    /// Author date as git's raw `%at` (unix seconds), compared as text.
    pub author_timestamp: String,
    pub subject: String,
}

/// A `commits` row whose SHA is no longer on the branch.
#[derive(Debug, Clone)]
pub struct OrphanedRow {
    pub row_id: String,
    pub old_sha: String,
    pub identity: CommitIdentity,
}

/// A commit currently on the branch, as a candidate for an orphaned row.
#[derive(Debug, Clone)]
pub struct RewrittenCommit {
    pub sha: String,
    pub identity: CommitIdentity,
    /// Whether a `commits` row already owns this SHA. Claimed commits are
    /// never handed to an orphaned row — the existing row wins.
    pub claimed: bool,
}

/// A row to repoint, produced by [`match_rewritten_commits`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaRemap {
    pub row_id: String,
    pub old_sha: String,
    pub new_sha: String,
}

/// `%H`, `%ae`, `%at`, `%s`, separated by `%x1f` (the unit separator, as in
/// `BRANCH_COMMIT_LOG_FORMAT`) since git technically permits `|` in emails;
/// the subject still goes last as the remainder. Deliberately not
/// `CommitInfo`'s format, which carries `%ct` (committer time), the one
/// timestamp a rebase rewrites.
const REASSOCIATION_LOG_FORMAT: &str = "--format=%H%x1f%ae%x1f%at%x1f%s";

/// Pair orphaned rows with the commits that replaced them.
///
/// Both inputs must be in branch order, oldest first: when several commits
/// share an identity (two `wip` commits authored in the same second, say),
/// they're paired oldest-to-oldest. Unmatched rows are simply left out — the
/// caller leaves them orphaned, which is what happens today anyway.
pub fn match_rewritten_commits(
    orphans: &[OrphanedRow],
    rewritten: &[RewrittenCommit],
) -> Vec<ShaRemap> {
    let mut available: HashMap<&CommitIdentity, VecDeque<&str>> = HashMap::new();
    for commit in rewritten.iter().filter(|c| !c.claimed) {
        available
            .entry(&commit.identity)
            .or_default()
            .push_back(&commit.sha);
    }

    let mut remaps = Vec::new();
    for orphan in orphans {
        let Some(candidates) = available.get_mut(&orphan.identity) else {
            continue;
        };
        let Some(new_sha) = candidates.pop_front() else {
            continue;
        };
        remaps.push(ShaRemap {
            row_id: orphan.row_id.clone(),
            old_sha: orphan.old_sha.clone(),
            new_sha: new_sha.to_string(),
        });
    }
    remaps
}

/// Repoint a branch's orphaned commit rows (and their reviews) at the SHAs a
/// rebase rewrote them into. Returns how many rows were remapped.
///
/// Safe to call when nothing was rewritten: with no orphaned rows it stops
/// after listing the branch and returns 0. Also safe to call while a rebase is
/// still in flight — it returns 0 without touching a row (see
/// [`head_is_on_branch`]).
pub fn reassociate_after_rebase(
    store: &Store,
    branch_id: &str,
    working_dir: &Path,
    workspace_name: Option<&str>,
) -> Result<usize, String> {
    let branch = store
        .get_branch(branch_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Branch not found: {branch_id}"))?;

    let repo_subpath = match workspace_name {
        Some(_) => crate::branches::resolve_branch_workspace_subpath(store, &branch)?,
        None => None,
    };
    let git = |args: &[&str]| -> Result<String, String> {
        match workspace_name {
            Some(ws_name) => {
                crate::branches::run_workspace_git(ws_name, repo_subpath.as_deref(), args)
                    .map_err(|e| e.to_string())
            }
            None => crate::git::cli_run_smart(working_dir, args).map_err(|e| e.to_string()),
        }
    };

    let branch_name = crate::git::branch_name_without_origin(&branch.branch_name);
    if !head_is_on_branch(&git, branch_name) {
        log::warn!(
            "Skipping commit reassociation on branch {branch_id}: HEAD isn't on {branch_name} \
             (rebase still in progress?)"
        );
        return Ok(0);
    }

    let base_ref = crate::git::origin_ref_for_branch(&branch.base_branch);
    let on_branch = list_branch_commits(&git, &base_ref)?;

    // `list_commits_for_branch` orders by `created_at`, which is the order the
    // sessions authored them — i.e. branch order, as the matcher requires.
    let rows = store
        .list_commits_for_branch(branch_id)
        .map_err(|e| e.to_string())?;
    let owned: HashSet<&str> = rows.iter().filter_map(|row| row.sha.as_deref()).collect();

    let rewritten: Vec<RewrittenCommit> = on_branch
        .into_iter()
        .map(|(sha, identity)| RewrittenCommit {
            claimed: owned.contains(sha.as_str()),
            sha,
            identity,
        })
        .collect();

    let still_on_branch: HashSet<&str> = rewritten.iter().map(|c| c.sha.as_str()).collect();
    let orphan_shas: Vec<String> = owned
        .iter()
        .filter(|sha| !still_on_branch.contains(*sha))
        .map(|sha| (*sha).to_string())
        .collect();
    if orphan_shas.is_empty() {
        return Ok(0);
    }

    let mut old_identities = lookup_commit_identities(&git, &orphan_shas)?;
    let orphans: Vec<OrphanedRow> = rows
        .iter()
        .filter_map(|row| {
            let old_sha = row.sha.clone()?;
            // GC-pruned objects drop out here, leaving their row orphaned.
            let identity = old_identities.remove(&old_sha)?;
            Some(OrphanedRow {
                row_id: row.id.clone(),
                old_sha,
                identity,
            })
        })
        .collect();

    let remaps = match_rewritten_commits(&orphans, &rewritten);
    if remaps.is_empty() {
        return Ok(0);
    }

    let pairs: Vec<(&str, &str, &str)> = remaps
        .iter()
        .map(|r| (r.row_id.as_str(), r.old_sha.as_str(), r.new_sha.as_str()))
        .collect();
    store
        .remap_commit_shas(branch_id, &pairs)
        .map_err(|e| e.to_string())
}

/// Whether HEAD is the branch we're about to reassociate, rather than a
/// detached commit.
///
/// This is how we tell a finished rebase from one still in flight: a rebase
/// detaches HEAD for the whole rewrite and only moves the branch ref at the
/// end. If the agent stops with conflicts unresolved — or its turn simply ends
/// — HEAD sits on a partially applied commit, and repointing rows there would
/// be strictly worse than leaving them orphaned: a later `git rebase --abort`
/// restores the original SHAs, and the rows (plus their reviews) would then
/// name commits that are on no branch at all. Any other detached or
/// wrong-branch HEAD is skipped for the same reason — `merge-base..HEAD` isn't
/// the branch's history, so nothing it lists can be trusted as a rewrite of it.
fn head_is_on_branch<F>(git: &F, branch_name: &str) -> bool
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    // Exits non-zero on a detached HEAD; a failure for any other reason also
    // reads as "don't touch anything", which is the safe direction.
    match git(&["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(head) => head.trim() == branch_name,
        Err(_) => false,
    }
}

/// List the branch's commits as `(sha, identity)` pairs, oldest first.
fn list_branch_commits<F>(git: &F, base_ref: &str) -> Result<Vec<(String, CommitIdentity)>, String>
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    // Fall back to the bare base ref the way the timeline does, so a repo
    // without a shared history with `origin/{base}` still reports something.
    let range = match git(&["merge-base", base_ref, "HEAD"]) {
        Ok(output) if !output.trim().is_empty() => format!("{}..HEAD", output.trim()),
        _ => format!("{base_ref}..HEAD"),
    };
    let output = git(&["log", REASSOCIATION_LOG_FORMAT, &range, "--"])?;

    // `git log` is newest-first; the matcher wants branch order, oldest first.
    Ok(output
        .lines()
        .filter_map(parse_identity_line)
        .rev()
        .collect())
}

/// Batch-read metadata for commits that are no longer on any branch. The old
/// objects survive a rebase (the reflog keeps them alive), and
/// `--ignore-missing` silently drops any that have since been GC-pruned.
fn lookup_commit_identities<F>(
    git: &F,
    shas: &[String],
) -> Result<HashMap<String, CommitIdentity>, String>
where
    F: Fn(&[&str]) -> Result<String, String>,
{
    // Guard the empty case: `git log --no-walk` with no revisions defaults to
    // HEAD, which would hand back metadata for a commit nobody asked about.
    if shas.is_empty() {
        return Ok(HashMap::new());
    }

    let mut args = vec![
        "log",
        "--no-walk=unsorted",
        "--ignore-missing",
        REASSOCIATION_LOG_FORMAT,
    ];
    args.extend(shas.iter().map(String::as_str));
    args.push("--");
    let output = git(&args)?;

    Ok(output.lines().filter_map(parse_identity_line).collect())
}

/// Parse one [`REASSOCIATION_LOG_FORMAT`] line. The subject is the remainder,
/// so even a subject containing the separator byte survives intact.
fn parse_identity_line(line: &str) -> Option<(String, CommitIdentity)> {
    let mut parts = line.splitn(4, '\x1f');
    let sha = parts.next()?;
    let author_email = parts.next()?;
    let author_timestamp = parts.next()?;
    let subject = parts.next()?;
    if sha.is_empty() {
        return None;
    }
    Some((
        sha.to_string(),
        CommitIdentity {
            author_email: author_email.to_string(),
            author_timestamp: author_timestamp.to_string(),
            subject: subject.to_string(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(email: &str, timestamp: &str, subject: &str) -> CommitIdentity {
        CommitIdentity {
            author_email: email.to_string(),
            author_timestamp: timestamp.to_string(),
            subject: subject.to_string(),
        }
    }

    fn orphan(row_id: &str, old_sha: &str, identity: CommitIdentity) -> OrphanedRow {
        OrphanedRow {
            row_id: row_id.to_string(),
            old_sha: old_sha.to_string(),
            identity,
        }
    }

    fn rewritten(sha: &str, identity: CommitIdentity) -> RewrittenCommit {
        RewrittenCommit {
            sha: sha.to_string(),
            identity,
            claimed: false,
        }
    }

    #[test]
    fn matches_rewritten_commits_by_author_identity() {
        let parser = identity("a@example.com", "100", "feat: parser");
        let lexer = identity("b@example.com", "200", "fix: lexer");

        let remaps = match_rewritten_commits(
            &[
                orphan("row-1", "abc111", parser.clone()),
                orphan("row-2", "abc222", lexer.clone()),
            ],
            &[rewritten("def444", parser), rewritten("def555", lexer)],
        );

        assert_eq!(
            remaps,
            vec![
                ShaRemap {
                    row_id: "row-1".into(),
                    old_sha: "abc111".into(),
                    new_sha: "def444".into()
                },
                ShaRemap {
                    row_id: "row-2".into(),
                    old_sha: "abc222".into(),
                    new_sha: "def555".into()
                },
            ]
        );
    }

    /// A conflict-resolved commit has different content but the same author
    /// metadata, so it still matches — that's the whole point of the key.
    #[test]
    fn matches_commit_whose_content_changed_during_conflict_resolution() {
        let resolved = identity("a@example.com", "100", "feat: parser");
        let remaps = match_rewritten_commits(
            &[orphan("row-1", "abc111", resolved.clone())],
            &[rewritten("def444", resolved)],
        );
        assert_eq!(remaps.len(), 1);
        assert_eq!(remaps[0].new_sha, "def444");
    }

    /// Two commits authored in the same second with the same subject are
    /// indistinguishable by key, so they pair up in branch order.
    #[test]
    fn matches_duplicate_identities_oldest_to_oldest() {
        let wip = identity("a@example.com", "100", "wip");

        let remaps = match_rewritten_commits(
            &[
                orphan("row-old", "abc111", wip.clone()),
                orphan("row-new", "abc222", wip.clone()),
            ],
            &[rewritten("def444", wip.clone()), rewritten("def555", wip)],
        );

        assert_eq!(remaps[0].row_id, "row-old");
        assert_eq!(remaps[0].new_sha, "def444");
        assert_eq!(remaps[1].row_id, "row-new");
        assert_eq!(remaps[1].new_sha, "def555");
    }

    /// A commit the rebase dropped (it became empty) has no counterpart; its
    /// row stays orphaned rather than stealing a neighbour's SHA.
    #[test]
    fn leaves_dropped_commit_unmatched() {
        let kept = identity("a@example.com", "100", "feat: parser");
        let dropped = identity("a@example.com", "200", "chore: already upstream");

        let remaps = match_rewritten_commits(
            &[
                orphan("row-kept", "abc111", kept.clone()),
                orphan("row-dropped", "abc222", dropped),
            ],
            &[rewritten("def444", kept)],
        );

        assert_eq!(remaps.len(), 1);
        assert_eq!(remaps[0].row_id, "row-kept");
    }

    /// A rewritten commit that already has a row of its own is off-limits —
    /// e.g. the rebase session's own pending row once it has landed.
    #[test]
    fn skips_rewritten_commits_that_already_have_a_row() {
        let parser = identity("a@example.com", "100", "feat: parser");
        let mut claimed = rewritten("def444", parser.clone());
        claimed.claimed = true;

        let remaps = match_rewritten_commits(&[orphan("row-1", "abc111", parser)], &[claimed]);

        assert!(remaps.is_empty());
    }

    /// A `|` is ordinary text in every field now that the separator is
    /// `%x1f`; only a separator byte in the subject needs the remainder rule.
    #[test]
    fn parses_pipes_and_trailing_separators_intact() {
        let (sha, identity) =
            parse_identity_line("abc111\x1fa|b@example.com\x1f100\x1fchore: rename a\x1fb to c")
                .unwrap();
        assert_eq!(sha, "abc111");
        assert_eq!(identity.author_email, "a|b@example.com");
        assert_eq!(identity.subject, "chore: rename a\x1fb to c");
        assert_eq!(identity.author_timestamp, "100");
    }

    #[test]
    fn ignores_malformed_log_lines() {
        assert!(parse_identity_line("").is_none());
        assert!(parse_identity_line("abc111\x1fa@example.com\x1f100").is_none());
    }

    /// Mid-rebase, `symbolic-ref` exits non-zero because HEAD is detached; a
    /// checkout of some other branch answers with its name. Neither is the
    /// branch we were asked to reassociate.
    #[test]
    fn head_is_on_branch_requires_an_attached_matching_head() {
        let on_feature = |_: &[&str]| -> Result<String, String> { Ok("feature\n".to_string()) };
        assert!(head_is_on_branch(&on_feature, "feature"));
        assert!(!head_is_on_branch(&on_feature, "other"));

        let detached = |_: &[&str]| -> Result<String, String> {
            Err("fatal: ref HEAD is not a symbolic ref".to_string())
        };
        assert!(!head_is_on_branch(&detached, "feature"));
    }
}
