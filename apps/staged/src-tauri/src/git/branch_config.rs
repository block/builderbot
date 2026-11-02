//! One `git config` read for everything a branch timeline build needs from
//! config: the branch's upstream entries and the user's identity.
//!
//! The timeline used to ask for these one key at a time — `branch.<b>.remote`,
//! `branch.<b>.merge`, `user.name`, `user.email` — four process spawns on the
//! first-paint path. A single `git config -z --get-regexp` returns them all,
//! and git lists scopes from lowest to highest precedence, so the last value
//! seen for a key is the one `git config <key>` would have returned.

use std::path::Path;

use super::cli::{self, GitError, GitOutput};
use super::upstream::upstream_branch_name_from_config;

/// The config entries a timeline build reads, as of one `git config` call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchConfigSnapshot {
    /// `branch.<local>.remote`, `None` when unset or empty.
    pub upstream_remote: Option<String>,
    /// `branch.<local>.merge`, `None` when unset or empty.
    pub upstream_merge: Option<String>,
    /// `user.name`, `None` when unset or empty.
    pub user_name: Option<String>,
    /// `user.email`, `None` when unset or empty.
    pub user_email: Option<String>,
}

impl BranchConfigSnapshot {
    /// The branch on `origin` that `local` pushes to — the same decision as
    /// [`super::upstream_branch_name`], from the entries already read.
    pub fn upstream_branch_name(&self, local: &str, base_branch: &str) -> String {
        upstream_branch_name_from_config(
            self.upstream_remote.as_deref(),
            self.upstream_merge.as_deref(),
            local,
            base_branch,
        )
    }
}

/// Read `branch.<local>.*` and `user.*` from `worktree`'s config in one spawn.
///
/// Config reads only touch `.git/config` and the user's global config, so the
/// lite env is enough — and required: `git config --get-regexp` exits 1 when
/// nothing matches, which the retrying runner would mistake for an
/// env-sensitive failure and answer with a blocking shell-env capture. A
/// no-match exit is an empty snapshot; any other failure is returned so the
/// caller can fall back to what it last knew.
pub fn read_branch_config_snapshot(
    worktree: &Path,
    local: &str,
) -> Result<BranchConfigSnapshot, GitError> {
    let pattern = branch_config_regexp(local);
    let output = cli::run_lite_output(worktree, &["config", "-z", "--get-regexp", &pattern])?;
    snapshot_from_output(&output, local)
}

/// Read the snapshot off a finished `git config -z --get-regexp` run.
///
/// Exit 0 is the entries. Exit 1 with nothing on stderr is git's "no key
/// matched" and an empty snapshot: the repo really has no identity and no
/// upstream entries, and the caller may take that as the answer. Everything
/// else — an unreadable config file (exit 3 or 128), a child killed before it
/// exited, an exit 1 that came with a complaint — is a failed read, returned
/// as an error so the caller keeps what it last knew rather than forgetting
/// the user's identity for a paint.
fn snapshot_from_output(output: &GitOutput, local: &str) -> Result<BranchConfigSnapshot, GitError> {
    if output.success {
        return Ok(snapshot_from_entries(
            &parse_config_entries_z(&output.stdout),
            local,
        ));
    }
    let stderr = output.stderr.trim();
    if output.code == Some(1) && stderr.is_empty() {
        return Ok(BranchConfigSnapshot::default());
    }
    Err(GitError::CommandFailed(if stderr.is_empty() {
        match output.code {
            Some(code) => format!("git config exited with status {code}"),
            None => "git config was killed by a signal".to_string(),
        }
    } else {
        stderr.to_string()
    }))
}

/// `^(branch\.<local>\.|user\.)`, with `local` escaped for the POSIX extended
/// regex `git config --get-regexp` compiles. Ref names may contain `.`, `+`,
/// `(`, `)`, `{`, `}`, `|` and `$`, all of which mean something to the regex.
pub(crate) fn branch_config_regexp(local: &str) -> String {
    let mut escaped = String::with_capacity(local.len());
    for ch in local.chars() {
        if matches!(
            ch,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    format!("^(branch\\.{escaped}\\.|user\\.)")
}

/// Parse `git config -z --get-regexp` output: each entry is `key\nvalue\0`,
/// or `key\0` for a key set without a value. Entries that don't fit — a
/// trailing fragment with no terminator, an empty key — are skipped.
pub(crate) fn parse_config_entries_z(raw: &str) -> Vec<(String, Option<String>)> {
    let mut entries = Vec::new();
    let mut parts = raw.split('\0').peekable();
    while let Some(entry) = parts.next() {
        // The output ends in `\0`, so the final split is an empty tail; a
        // non-empty tail is a fragment with no terminator and is dropped.
        if parts.peek().is_none() {
            break;
        }
        let (key, value) = match entry.split_once('\n') {
            Some((key, value)) => (key, Some(value.to_string())),
            None => (entry, None),
        };
        if key.is_empty() {
            continue;
        }
        entries.push((key.to_string(), value));
    }
    entries
}

fn snapshot_from_entries(
    entries: &[(String, Option<String>)],
    local: &str,
) -> BranchConfigSnapshot {
    let remote_key = format!("branch.{local}.remote");
    let merge_key = format!("branch.{local}.merge");
    let mut snapshot = BranchConfigSnapshot::default();
    // Later entries come from higher-precedence scopes and win, as they do for
    // `git config <key>`.
    for (key, value) in entries {
        let value = value
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        if *key == remote_key {
            snapshot.upstream_remote = value;
        } else if *key == merge_key {
            snapshot.upstream_merge = value;
        } else if key == "user.name" {
            snapshot.user_name = value;
        } else if key == "user.email" {
            snapshot.user_email = value;
        }
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TempGitRepo;

    fn entries(raw: &str) -> Vec<(String, Option<String>)> {
        parse_config_entries_z(raw)
    }

    fn entry(key: &str, value: Option<&str>) -> (String, Option<String>) {
        (key.to_string(), value.map(str::to_string))
    }

    #[test]
    fn parses_nul_terminated_entries_in_order() {
        let raw = "user.name\nGlobal Name\0user.email\nglobal@example.com\0branch.feature.remote\norigin\0branch.feature.merge\nrefs/heads/feature\0user.name\nLocal Name\0";
        assert_eq!(
            entries(raw),
            vec![
                entry("user.name", Some("Global Name")),
                entry("user.email", Some("global@example.com")),
                entry("branch.feature.remote", Some("origin")),
                entry("branch.feature.merge", Some("refs/heads/feature")),
                entry("user.name", Some("Local Name")),
            ]
        );
    }

    #[test]
    fn empty_output_is_no_entries() {
        assert!(parse_config_entries_z("").is_empty());
        assert!(parse_config_entries_z("\0").is_empty());
    }

    #[test]
    fn malformed_output_is_skipped_not_misread() {
        // A key set with no value, a value-less terminator, an empty key, and
        // a trailing fragment with no terminator.
        let raw = "user.name\0user.email\n\0\0\nstray\0branch.feature.remote\norigin";
        assert_eq!(
            entries(raw),
            vec![entry("user.name", None), entry("user.email", Some(""))]
        );
        // Plain `--get-regexp` output without `-z` has no terminators at all.
        assert!(parse_config_entries_z("user.name Matt\nuser.email m@example.com\n").is_empty());
    }

    #[test]
    fn snapshot_takes_the_highest_precedence_value_and_drops_empties() {
        let raw = "user.name\nGlobal Name\0user.email\nglobal@example.com\0branch.feature.remote\norigin\0branch.feature.merge\nrefs/heads/feature-x\0user.name\nLocal Name\0user.email\n\0branch.other.remote\nfork\0";
        let snapshot = snapshot_from_entries(&parse_config_entries_z(raw), "feature");
        assert_eq!(
            snapshot,
            BranchConfigSnapshot {
                upstream_remote: Some("origin".into()),
                upstream_merge: Some("refs/heads/feature-x".into()),
                user_name: Some("Local Name".into()),
                user_email: None,
            }
        );
        assert_eq!(
            snapshot.upstream_branch_name("feature", "main"),
            "feature-x"
        );
    }

    #[test]
    fn regexp_escapes_the_branch_name() {
        assert_eq!(
            branch_config_regexp("release/1.2+hotfix(a)"),
            "^(branch\\.release/1\\.2\\+hotfix\\(a\\)\\.|user\\.)"
        );
        assert_eq!(branch_config_regexp("main"), "^(branch\\.main\\.|user\\.)");
    }

    /// One real read: the identity the repo sets plus an upstream, then the
    /// no-upstream shape, then a branch whose name needs escaping.
    #[test]
    fn reads_identity_and_upstream_from_a_real_repo() {
        let repo = TempGitRepo::new();
        repo.write_file("file.txt", "base\n");
        repo.commit("base");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.run_git(&["checkout", "-b", "feature-x-2"]);
        repo.run_git(&["config", "branch.feature-x-2.remote", "origin"]);
        repo.run_git(&["config", "branch.feature-x-2.merge", "refs/heads/feature-x"]);

        let snapshot = read_branch_config_snapshot(repo.path(), "feature-x-2").unwrap();
        assert_eq!(snapshot.user_name.as_deref(), Some("Test"));
        assert_eq!(snapshot.user_email.as_deref(), Some("test@example.com"));
        assert_eq!(snapshot.upstream_remote.as_deref(), Some("origin"));
        assert_eq!(
            snapshot.upstream_merge.as_deref(),
            Some("refs/heads/feature-x")
        );
        assert_eq!(
            snapshot.upstream_branch_name("feature-x-2", "main"),
            "feature-x"
        );

        // No upstream configured: identity only, upstream is the local name.
        repo.run_git(&["checkout", "-b", "loose"]);
        let snapshot = read_branch_config_snapshot(repo.path(), "loose").unwrap();
        assert_eq!(snapshot.user_name.as_deref(), Some("Test"));
        assert!(snapshot.upstream_remote.is_none());
        assert_eq!(snapshot.upstream_branch_name("loose", "main"), "loose");

        // Upstream on another remote: read, but not a push target on origin.
        repo.run_git(&["config", "branch.loose.remote", "fork"]);
        repo.run_git(&["config", "branch.loose.merge", "refs/heads/loose"]);
        let snapshot = read_branch_config_snapshot(repo.path(), "loose").unwrap();
        assert_eq!(snapshot.upstream_remote.as_deref(), Some("fork"));
        assert_eq!(snapshot.upstream_branch_name("loose", "main"), "loose");

        // A dotted branch name must match itself, not `branch.vX1Y.*`.
        repo.run_git(&["checkout", "-b", "v1.2"]);
        repo.run_git(&["config", "branch.v1.2.remote", "origin"]);
        repo.run_git(&["config", "branch.v1.2.merge", "refs/heads/v1.2"]);
        repo.run_git(&["config", "branch.v1x2.remote", "fork"]);
        let snapshot = read_branch_config_snapshot(repo.path(), "v1.2").unwrap();
        assert_eq!(snapshot.upstream_remote.as_deref(), Some("origin"));
        assert_eq!(snapshot.upstream_branch_name("v1.2", "main"), "v1.2");
    }

    fn output(success: bool, code: Option<i32>, stdout: &str, stderr: &str) -> GitOutput {
        GitOutput {
            success,
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    /// Only git's own "no match" (exit 1, silent) is an empty snapshot. Any
    /// other failure is an error, including an exit 1 that complained, so a
    /// caller never mistakes a read it could not do for "no identity set".
    #[test]
    fn only_a_silent_exit_one_is_an_empty_snapshot() {
        let parsed = snapshot_from_output(
            &output(
                true,
                Some(0),
                "user.name\nMatt\0user.email\nm@example.com\0",
                "",
            ),
            "feature",
        )
        .unwrap();
        assert_eq!(parsed.user_name.as_deref(), Some("Matt"));
        assert_eq!(parsed.user_email.as_deref(), Some("m@example.com"));

        assert_eq!(
            snapshot_from_output(&output(false, Some(1), "", ""), "feature").unwrap(),
            BranchConfigSnapshot::default()
        );
        assert_eq!(
            snapshot_from_output(&output(false, Some(1), "", "  \n"), "feature").unwrap(),
            BranchConfigSnapshot::default()
        );

        let complained = snapshot_from_output(
            &output(
                false,
                Some(1),
                "",
                "error: key does not contain a section: x\n",
            ),
            "feature",
        )
        .unwrap_err();
        assert!(
            matches!(&complained, GitError::CommandFailed(msg) if msg.contains("does not contain a section")),
            "{complained:?}"
        );

        let bad_file = snapshot_from_output(
            &output(
                false,
                Some(128),
                "",
                "fatal: bad config line 1 in file .git/config\n",
            ),
            "feature",
        )
        .unwrap_err();
        assert!(
            matches!(&bad_file, GitError::CommandFailed(msg) if msg.contains("bad config line")),
            "{bad_file:?}"
        );

        // Silent failures that are not exit 1: the status is the message.
        let exit_three =
            snapshot_from_output(&output(false, Some(3), "", ""), "feature").unwrap_err();
        assert!(
            matches!(&exit_three, GitError::CommandFailed(msg) if msg == "git config exited with status 3"),
            "{exit_three:?}"
        );
        let killed = snapshot_from_output(&output(false, None, "", ""), "feature").unwrap_err();
        assert!(
            matches!(&killed, GitError::CommandFailed(msg) if msg.contains("killed by a signal")),
            "{killed:?}"
        );
    }

    /// A config file git cannot parse fails the read with git's message on
    /// stderr — not an empty snapshot that would read as "no identity".
    #[test]
    fn an_unreadable_config_file_is_an_error_not_an_empty_snapshot() {
        let repo = TempGitRepo::new();
        std::fs::write(repo.path().join(".git/config"), "[core\n").unwrap();
        let err = read_branch_config_snapshot(repo.path(), "feature").unwrap_err();
        assert!(
            matches!(&err, GitError::CommandFailed(msg) if msg.contains("bad config line")),
            "{err:?}"
        );
    }

    /// A repo with no identity and no upstream: `--get-regexp` exits 1 with
    /// nothing on stderr, which is an empty snapshot, not an error.
    #[test]
    fn no_matching_entries_is_an_empty_snapshot() {
        let repo = TempGitRepo::new();
        repo.run_git(&["config", "--unset", "user.name"]);
        repo.run_git(&["config", "--unset", "user.email"]);
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(repo.path())
            .args(["config", "--global", "--get-regexp", "^user\\."]);
        crate::git::strip_git_env(&mut command);
        let global_has_identity = command
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if global_has_identity {
            // The developer's global identity would be read; the empty-output
            // path is covered by `empty_output_is_no_entries`.
            return;
        }
        let snapshot = read_branch_config_snapshot(repo.path(), "feature").unwrap();
        assert_eq!(snapshot, BranchConfigSnapshot::default());
    }
}
