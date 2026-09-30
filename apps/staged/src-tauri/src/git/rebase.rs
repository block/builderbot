//! Rebase a branch without losing content that exists only inside its merge
//! commits.
//!
//! `git rebase` replays the non-merge commits of `<onto>..HEAD` and drops the
//! merges. That is correct for a pure automatic merge, whose content is fully
//! described by its parents. It silently destroys an "evil merge": a merge
//! commit into which someone also typed changes beyond what the automatic merge
//! produced. Those hand edits are not a patch in the replayed list, so they
//! vanish, and every later commit that built on them either conflicts or, worse,
//! applies cleanly against a tree that no longer has them.
//!
//! [`rebase_preserving_merge_edits_command`] is the one place Staged builds a
//! rebase from. Every pipeline that rebases (rebase onto base, rebase onto
//! origin, and any future variant) must go through it. The command it returns
//! is a self-contained POSIX `sh` program, because pipeline steps run as shell
//! commands both locally and on remote workspaces, and the safeguard has to
//! behave identically in both places.
//!
//! What the command does, before touching the branch:
//!
//! 1. Lists the merge commits in `<onto>..HEAD`, oldest first in topological
//!    order. A branch with none takes the plain `git rebase --signoff` path and
//!    works on any git version.
//! 2. Checks `git --version` against [`REQUIRED_GIT_VERSION`]. `git merge-tree
//!    --write-tree`, which the next step needs, arrived in git 2.38; an older
//!    git cannot tell which merges carry hand edits, so the rebase is refused
//!    with a message naming the installed and required versions rather than
//!    risking dropped content.
//! 3. For each merge, computes the automatic merge of its two parents with
//!    `git merge-tree --write-tree` and compares that tree with the merge's real
//!    tree. Equal trees mean a pure automatic merge, dropped exactly as before.
//!    Any other failure of `merge-tree` (unrelated histories, missing objects
//!    in a shallow clone) stops the command with git's own stderr.
//! 4. For a merge whose trees differ, records two throwaway commits with
//!    `git commit-tree`: the automatic merge result, and on top of it the
//!    merge's real tree, authored by the merge's original author at the
//!    original author date. The second commit's diff against its parent is
//!    precisely the hand edits, and its message names the merge it came from.
//! 5. Runs `git rebase --interactive` with a `GIT_SEQUENCE_EDITOR` that inserts a
//!    `pick` of each hand-edit commit right after the last surviving pick that
//!    is an ancestor of the merge, so it lands where the merge sat in the
//!    sequence and later commits that depend on it still apply. When
//!    `rebase.updateRefs` is on, git writes `update-ref refs/heads/<x>` lines
//!    directly after the pick a stacked branch points at; the insert goes after
//!    those lines so the stacked branch keeps following its own commit. Before
//!    handing the todo back, the editor checks that every inserted pick is
//!    present in it, verbatim, and exits non-zero naming any that are not; git
//!    then aborts without having touched the branch. A damaged splice can
//!    therefore never produce a completed rebase that lacks the hand edits.
//!
//! If a hand-edit commit does not apply cleanly the rebase stops on it like any
//! other conflict, and the command names the merge the content came from. Its
//! output doubles as the summary shown in the pipeline step: which merges were
//! linearised and which files their edits touched, which pure automatic merges
//! were dropped, and the resulting commit list.
//!
//! # Shape of the generated command
//!
//! The command is a single line. On a remote workspace a pipeline step travels
//! as one argv element through `sq blox ws exec … -- sh -c "cd '…' && sh -lc
//! '<command>'"`, and nothing in this repository can verify that an embedded
//! newline survives that channel. So [`STATEMENTS`] holds complete `sh`
//! commands that are joined with `; `, and the two helper programs the rebase
//! needs as files, the sequence editor ([`EDITOR_LINES`]) and its awk splice
//! ([`SPLICE_AWK_LINES`]), are written at run time with `printf '%s\n'` of
//! single-quoted lines. A newline only ever appears as the two characters `\n`
//! inside a `printf` format. Each write fails the command if it does not
//! complete, and before the rebase starts the editor is syntax-checked with
//! `sh -n` and its line count compared with the source, so a damaged or
//! truncated helper fails loudly instead of producing a wrong todo. The line
//! count matters because the editor's own presence check (step 5) is part of
//! the editor: a copy cut short before the splice would also lose the check.
//! Everything is plain POSIX `sh`, tested under both `sh` and `dash`.
//!
//! The version check lives in the command rather than reusing the Rust-side
//! probes in `git::config_apply`. Those probe the git on the app's own PATH, or
//! run a separate `ws_exec`; the command checks the git that will actually run
//! the rebase, in the pipeline's login-shell environment, on whichever machine
//! the branch lives, and it already knows whether the range has merges at all.
//!
//! `--rebase-merges` is deliberately not used. It recreates merges by
//! re-running them, so hand edits are lost anyway, and a merge whose second
//! parent is already an ancestor of the new base degenerates to a no-op.

/// Token in [`STATEMENTS`] that [`rebase_preserving_merge_edits_command`]
/// replaces with the shell-quoted ref to rebase onto.
const ONTO_PLACEHOLDER: &str = "__STAGED_ONTO__";

/// Token in [`STATEMENTS`] replaced with [`REQUIRED_GIT_VERSION`].
const REQUIRED_GIT_PLACEHOLDER: &str = "__STAGED_REQUIRED_GIT__";

/// Token in [`STATEMENTS`] replaced with the statements that write the sequence
/// editor and its awk splice into the temporary directory.
const WRITE_HELPERS_PLACEHOLDER: &str = "__STAGED_WRITE_HELPERS__";

/// Token in [`STATEMENTS`] replaced with the number of lines in
/// [`EDITOR_LINES`], which the written editor must have before it is used.
const EDITOR_LINE_COUNT_PLACEHOLDER: &str = "__STAGED_EDITOR_LINE_COUNT__";

/// The oldest git whose `merge-tree --write-tree` exists. Below it the command
/// refuses to rebase a branch that has merge commits.
pub const REQUIRED_GIT_VERSION: &str = "2.38";

/// Subject prefix of the commits that carry a merge's hand edits. The todo
/// editor and the post-rebase summary both key off it, and the AI handoff
/// prompt in `prs` tells the agent never to drop commits that start with it.
pub const HAND_EDIT_COMMIT_SUBJECT_PREFIX: &str = "Preserve hand edits from merge ";

/// The safeguard, as complete POSIX `sh` commands that are joined with `; `.
/// See the module docs for the algorithm. Everything the command prints goes to
/// stdout (git's own output is redirected there too) so the pipeline step shows
/// one ordered transcript.
///
/// Each element must be a complete command: compound commands (`if`, `for`,
/// function bodies) are written on one line with their own `;` separators.
const STATEMENTS: &[&str] = &[
    "onto=__STAGED_ONTO__",
    "required_git=__STAGED_REQUIRED_GIT__",
    "fail() { printf 'staged-rebase: %s\\n' \"$*\" >&2; exit 1; }",
    "git rev-parse --verify --quiet \"$onto^{commit}\" >/dev/null || fail \"cannot resolve $onto\"",
    "git rev-parse --verify --quiet 'HEAD^{commit}' >/dev/null || fail \"cannot resolve HEAD\"",
    "merges=$(git rev-list --reverse --topo-order --merges \"$onto..HEAD\") || fail \"cannot list the merge commits in $onto..HEAD\"",
    // No merges: the plain rebase, on any git version.
    "if [ -z \"$merges\" ]; then printf 'Merge commits in %s..HEAD: none\\n\\n' \"$onto\"; exec git rebase --signoff \"$onto\"; fi",
    "set -- $merges",
    "merge_count=$#",
    // Version pre-flight: refuse, before touching anything, when this git has no
    // `merge-tree --write-tree`. An unparseable version string is noted and
    // allowed through; a genuinely missing subcommand still fails loudly below
    // with git's own error, never by silently dropping content.
    "git_version=$(git --version 2>&1) || fail \"cannot run git --version: $git_version\"",
    concat!(
        "set -- $(printf '%s\\n' \"$git_version\" | ",
        "sed -n 's/^git version \\([0-9][0-9]*\\)\\.\\([0-9][0-9]*\\).*$/\\1 \\2/p')"
    ),
    concat!(
        "if [ $# -ne 2 ]; then ",
        "printf 'note: cannot parse the git version from \"%s\"; assuming git %s or newer\\n' \"$git_version\" \"$required_git\"; ",
        "elif [ \"$1\" -lt \"${required_git%.*}\" ] || { [ \"$1\" -eq \"${required_git%.*}\" ] && [ \"$2\" -lt \"${required_git#*.}\" ]; }; then ",
        "fail \"refusing to rebase with $git_version: git $required_git or newer is required to rebase a branch that has merge commits. ",
        "$onto..HEAD has $merge_count merge commit(s), and this git cannot tell which of them carry hand edits (it lacks git merge-tree --write-tree), ",
        "so a rebase could drop content that exists only inside those merge commits. The branch was left untouched; upgrade git and retry.\"; ",
        "fi"
    ),
    "tmp=$(mktemp -d \"${TMPDIR:-/tmp}/staged-rebase.XXXXXX\") || fail \"cannot create a temporary directory\"",
    "trap 'rm -rf \"$tmp\"' EXIT",
    WRITE_HELPERS_PLACEHOLDER,
    "sh -n \"$tmp/edit-todo\" || fail \"the generated todo editor failed a shell syntax check; refusing to start the rebase\"",
    // The editor's own check that the inserted picks reached the todo is its
    // final statements, so a copy truncated at a line boundary would pass
    // `sh -n` and silently skip both the splice and the check.
    concat!(
        "[ \"$(grep -c '' \"$tmp/edit-todo\")\" -eq __STAGED_EDITOR_LINE_COUNT__ ] || ",
        "fail \"the generated todo editor is incomplete ($(grep -c '' \"$tmp/edit-todo\") of __STAGED_EDITOR_LINE_COUNT__ lines); refusing to start the rebase\""
    ),
    ": > \"$tmp/synthetic\"",
    "linearised=0",
    "dropped=0",
    "printf 'Merge commits in %s..HEAD:\\n' \"$onto\"",
    concat!(
        "for merge in $merges; do ",
        "set -- $(git rev-list --parents -n 1 \"$merge\"); shift; ",
        "short=$(git rev-parse --short \"$merge\"); ",
        "subject=$(git log -1 --format=%s \"$merge\"); ",
        "[ $# -eq 2 ] || fail \"merge $short has $# parents; only two-parent merges can be linearised. Rebase this branch by hand.\"; ",
        // Exit 0: clean automatic merge. Exit 1: automatic merge with conflicts,
        // still a tree. Anything else is a real failure, reported as such.
        "auto=$(git merge-tree --write-tree \"$1\" \"$2\" 2>\"$tmp/merge-tree.err\"); merge_tree_status=$?; ",
        "if [ $merge_tree_status -gt 1 ]; then ",
        "fail \"cannot compute the automatic merge of the parents of merge $short (git merge-tree --write-tree exited $merge_tree_status): $(cat \"$tmp/merge-tree.err\")\"; ",
        "fi; ",
        "auto_tree=$(printf '%s\\n' \"$auto\" | sed -n 1p); ",
        "tree=$(git rev-parse --verify \"$merge^{tree}\") || fail \"cannot read the tree of merge $short\"; ",
        "if [ \"$auto_tree\" = \"$tree\" ]; then ",
        "dropped=$((dropped + 1)); ",
        "printf '  %s %s\\n    pure automatic merge: dropped\\n' \"$short\" \"$subject\"; ",
        "continue; ",
        "fi; ",
        "linearised=$((linearised + 1)); ",
        "files=$(git diff-tree -r --name-only \"$auto_tree\" \"$tree\"); ",
        "author_name=$(git log -1 --format=%an \"$merge\"); ",
        "author_email=$(git log -1 --format=%ae \"$merge\"); ",
        "author_date=$(git log -1 --format=%aD \"$merge\"); ",
        // Two throwaway commits: the automatic merge result, then the merge's
        // real tree on top of it. Picking the second replays exactly the hand
        // edits.
        "scaffold=$(GIT_AUTHOR_NAME=\"$author_name\" GIT_AUTHOR_EMAIL=\"$author_email\" GIT_AUTHOR_DATE=\"$author_date\" ",
        "git commit-tree \"$auto_tree\" -p \"$1\" -m \"Automatic merge result of $short (Staged rebase scaffolding)\") ",
        "|| fail \"cannot record the automatic merge result of merge $short\"; ",
        "message=$(printf 'Preserve hand edits from merge %s\\n\\nStaged linearised merge commit %s\\n(\"%s\") while rebasing onto %s. A rebase does not replay merge\\ncommits, so this commit carries the hand edits that merge introduced beyond\\nthe automatic merge of its parents; without it that content would be lost.\\n\\nFiles:\\n%s\\n' ",
        "\"$short\" \"$merge\" \"$subject\" \"$onto\" \"$(printf '%s\\n' \"$files\" | sed 's/^/  /')\"); ",
        "synthetic=$(GIT_AUTHOR_NAME=\"$author_name\" GIT_AUTHOR_EMAIL=\"$author_email\" GIT_AUTHOR_DATE=\"$author_date\" ",
        "git commit-tree \"$tree\" -p \"$scaffold\" -m \"$message\") ",
        "|| fail \"cannot record the hand edits of merge $short\"; ",
        "printf '%s %s %s\\n' \"$synthetic\" \"$merge\" \"$short\" >> \"$tmp/synthetic\"; ",
        "git rev-list \"$merge\" --not \"$onto\" > \"$tmp/ancestors.$merge\" || fail \"cannot list the ancestors of merge $short\"; ",
        "note=\"\"; ",
        "if [ $merge_tree_status -eq 1 ]; then note=\" (the automatic merge had conflicts; their resolution is part of the preserved edits)\"; fi; ",
        "printf '  %s %s\\n    hand edits kept as a separate commit by %s%s; files:\\n' \"$short\" \"$subject\" \"$author_name\" \"$note\"; ",
        "printf '%s\\n' \"$files\" | sed 's/^/      /'; ",
        "done"
    ),
    "printf '\\n'",
    concat!(
        "GIT_SEQUENCE_EDITOR=\"sh '$tmp/edit-todo'\" ",
        "git -c rebase.abbreviateCommands=false rebase --interactive --signoff --empty=drop --no-autosquash \"$onto\" 2>&1"
    ),
    "status=$?",
    concat!(
        "if [ $status -ne 0 ]; then ",
        "stopped=$(git rev-parse --verify --quiet REBASE_HEAD 2>/dev/null); ",
        "if [ -n \"$stopped\" ]; then ",
        "stopped_on=$(grep \"^$stopped \" \"$tmp/synthetic\" | cut -d' ' -f3); ",
        "if [ -n \"$stopped_on\" ]; then ",
        "printf '\\nThe rebase stopped while applying the hand edits carried by merge %s. That content exists in no other commit: resolve the conflict so it is kept, then continue the rebase with git rebase --continue. Do not drop this commit.\\n' \"$stopped_on\"; ",
        "fi; ",
        "fi; ",
        "exit $status; ",
        "fi"
    ),
    "printf '\\nRebase complete: %s merge commit(s) linearised, %s pure automatic merge(s) dropped.\\n' \"$linearised\" \"$dropped\"",
    concat!(
        "while read -r synthetic merge short; do ",
        "if ! git log --format=%s \"$onto..HEAD\" | grep -qFx \"Preserve hand edits from merge $short\"; then ",
        "printf 'The hand edits carried by merge %s were already present on %s; their commit became empty and was dropped.\\n' \"$short\" \"$onto\"; ",
        "fi; ",
        "done < \"$tmp/synthetic\""
    ),
    "printf '\\nCommits on the rebased branch:\\n'",
    "git log --reverse --format='  %h %s' \"$onto..HEAD\"",
];

/// Sequence editor for `git rebase -i`, one line per element. Written to
/// `$tmp/edit-todo` at run time and invoked by git as `sh "$tmp/edit-todo"
/// <todo>`, so `$0` is the file and `$(dirname "$0")` is `$tmp`.
///
/// It numbers the `pick` lines, finds for each hand-edit commit the last pick
/// that is an ancestor of its merge (0 when none survive), and hands the
/// resulting `<pick index>\t<todo line>` pairs to the awk splice. It then
/// re-reads the spliced todo and exits non-zero, naming each missing line,
/// unless every inserted pick is present verbatim. The check looks for the
/// exact inserted lines rather than counting subjects that look like
/// hand-edit commits, because a branch rebased this way before carries earlier
/// `Preserve hand edits from merge …` commits as ordinary picks.
const EDITOR_LINES: &[&str] = &[
    "todo=$1",
    "dir=$(dirname \"$0\")",
    ": > \"$dir/picks\"",
    "count=0",
    "while IFS= read -r line || [ -n \"$line\" ]; do",
    "  case \"$line\" in",
    // Long form only: the rebase runs with `rebase.abbreviateCommands=false`.
    "    \"pick \"*)",
    "      count=$((count + 1))",
    "      set -- $line",
    "      full=$(git rev-parse --verify --quiet \"$2^{commit}\" </dev/null) || exit 1",
    "      printf '%s %s\\n' \"$count\" \"$full\" >> \"$dir/picks\"",
    "      ;;",
    "  esac",
    "done < \"$todo\"",
    ": > \"$dir/inserts\"",
    ": > \"$dir/expected\"",
    "while read -r synthetic merge short; do",
    "  anchor=0",
    "  while read -r index full; do",
    "    if grep -qFx \"$full\" \"$dir/ancestors.$merge\"; then anchor=$index; fi",
    "  done < \"$dir/picks\"",
    "  insert=\"pick $synthetic Preserve hand edits from merge $short\"",
    "  printf '%s\\t%s\\n' \"$anchor\" \"$insert\" >> \"$dir/inserts\"",
    "  printf '%s\\n' \"$insert\" >> \"$dir/expected\"",
    "done < \"$dir/synthetic\"",
    "awk -v inserts=\"$dir/inserts\" -f \"$dir/splice.awk\" \"$todo\" > \"$todo.staged\" || exit 1",
    "mv \"$todo.staged\" \"$todo\" || exit 1",
    "missing=0",
    "while IFS= read -r insert; do",
    "  if ! grep -qFx \"$insert\" \"$todo\"; then",
    "    printf 'staged-rebase: preserved pick missing from the rebase todo: %s\\n' \"$insert\" >&2",
    "    missing=$((missing + 1))",
    "  fi",
    "done < \"$dir/expected\"",
    "if [ \"$missing\" -ne 0 ]; then",
    "  printf 'staged-rebase: %s of %s preserved hand-edit pick(s) did not reach the rebase todo, so the todo editor or its awk splice is damaged; refusing to start the rebase. The branch was left untouched.\\n' \"$missing\" \"$(grep -c '' \"$dir/expected\")\" >&2",
    "  exit 1",
    "fi",
];

/// The awk program that splices the inserts into the todo, one line per
/// element. Written to `$tmp/splice.awk` at run time.
///
/// Inserts keyed `0` go before everything. An insert keyed `n` is queued when
/// the `n`th pick is printed and emitted just before the next line that is not
/// an `update-ref`: with `rebase.updateRefs` on, git places `update-ref
/// refs/heads/<x>` directly after the pick that `<x>` points at, and emitting
/// the insert between them would repoint `<x>` at the hand-edit commit.
const SPLICE_AWK_LINES: &[&str] = &[
    "BEGIN {",
    "  while ((getline line < inserts) > 0) {",
    "    tab = index(line, \"\\t\")",
    "    key = substr(line, 1, tab - 1)",
    "    ins[key] = ins[key] substr(line, tab + 1) \"\\n\"",
    "  }",
    "  if (\"0\" in ins) printf \"%s\", ins[\"0\"]",
    "}",
    // Both rules match the long command forms only, and must stay symmetric:
    // the rebase runs with `rebase.abbreviateCommands=false`, so `p ` and
    // `u ` never appear, and matching one short form but not the other would
    // put inserts between a pick and its update-ref lines again.
    "pending != \"\" && !/^update-ref / { printf \"%s\", pending; pending = \"\" }",
    "{ print }",
    "/^pick / { n++; pending = \"\"; if ((n \"\") in ins) pending = ins[n \"\"] }",
    "END { if (pending != \"\") printf \"%s\", pending }",
];

/// The shell command that rebases the current branch onto `onto_ref` (for
/// example `origin/main`) while preserving hand edits carried by merge commits.
///
/// Signs off every replayed commit, as Staged's rebase always has. Prints a
/// summary of what happened to each merge commit, exits non-zero when the
/// rebase stops (on a conflict or otherwise), and names the merge a conflicting
/// hand-edit commit came from. The result is a single line of POSIX `sh` with
/// no raw newline, carriage return or tab. See the module docs for the full
/// behaviour.
pub fn rebase_preserving_merge_edits_command(onto_ref: &str) -> String {
    let write_helpers = format!(
        "{} || fail \"cannot write the todo editor to $tmp\"; {} || fail \"cannot write the todo splice to $tmp\"",
        write_file_statement("$tmp/edit-todo", EDITOR_LINES),
        write_file_statement("$tmp/splice.awk", SPLICE_AWK_LINES)
    );
    STATEMENTS
        .join("; ")
        .replace(WRITE_HELPERS_PLACEHOLDER, &write_helpers)
        .replace(
            EDITOR_LINE_COUNT_PLACEHOLDER,
            &EDITOR_LINES.len().to_string(),
        )
        .replace(REQUIRED_GIT_PLACEHOLDER, REQUIRED_GIT_VERSION)
        .replace(ONTO_PLACEHOLDER, &shell_single_quote(onto_ref))
}

/// A `printf '%s\n' <quoted line>... > <path>` statement that writes `lines`
/// as a file. Each line is a single-quoted word, so nothing in it is expanded
/// and no raw newline is needed in the command.
fn write_file_statement(path: &str, lines: &[&str]) -> String {
    let quoted: Vec<String> = lines.iter().map(|line| shell_single_quote(line)).collect();
    format!("printf '%s\\n' {} > \"{path}\"", quoted.join(" "))
}

/// Quote `value` as a single POSIX shell word.
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::strip_git_env;
    use crate::test_utils::TempGitRepo;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// What the rebase command produced: the pipeline sees the same two things,
    /// the exit status and the combined output.
    struct RebaseRun {
        success: bool,
        output: String,
    }

    /// How to run the command: which shell interprets it, an optional
    /// directory prepended to `PATH` so a fake `git` can stand in front of the
    /// real one, and optionally a different command to run in place of the
    /// generated one, for tests that damage it.
    #[derive(Default)]
    struct RunOptions<'a> {
        shell: Option<&'a str>,
        path_prefix: Option<&'a Path>,
        command: Option<String>,
    }

    /// Run the generated command the way a pipeline step does, `sh -c` in the
    /// worktree, isolated from the developer's own git config.
    fn run_rebase(repo: &TempGitRepo, onto: &str) -> RebaseRun {
        run_rebase_with(repo, onto, RunOptions::default())
    }

    fn run_rebase_with(repo: &TempGitRepo, onto: &str, options: RunOptions<'_>) -> RebaseRun {
        let mut command = Command::new(options.shell.unwrap_or("sh"));
        command
            .arg("-c")
            .arg(
                options
                    .command
                    .unwrap_or_else(|| rebase_preserving_merge_edits_command(onto)),
            )
            .current_dir(repo.path());
        strip_git_env(&mut command);
        command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        if let Some(prefix) = options.path_prefix {
            let mut paths = vec![prefix.to_path_buf()];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            command.env(
                "PATH",
                std::env::join_paths(paths).expect("PATH should join"),
            );
        }
        let output = command.output().expect("shell should spawn");
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        RebaseRun {
            success: output.status.success(),
            output: text,
        }
    }

    /// The shells the command must work under: `sh` always, and `dash` (the
    /// `/bin/sh` of Debian and Ubuntu, so of Blox workspaces) when installed.
    fn shells() -> Vec<&'static str> {
        let mut shells = vec!["sh"];
        if Command::new("dash")
            .arg("-c")
            .arg(":")
            .output()
            .is_ok_and(|output| output.status.success())
        {
            shells.push("dash");
        }
        shells
    }

    /// A directory holding a `git` wrapper script, for putting in front of the
    /// real git on `PATH`. `body` runs first with the original arguments; it
    /// falls through to the real git unless it exits.
    struct FakeGit {
        dir: PathBuf,
    }

    impl FakeGit {
        fn new(body: &str) -> Self {
            let real = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|dir| dir.join("git"))
                .find(|candidate| candidate.is_file())
                .expect("a real git on PATH");
            let dir =
                std::env::temp_dir().join(format!("staged-fake-git-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let script = format!(
                "#!/bin/sh\n{body}\nexec {} \"$@\"\n",
                shell_single_quote(&real.to_string_lossy())
            );
            let path = dir.join("git");
            std::fs::write(&path, script).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            Self { dir }
        }

        /// A git that reports `version` and is otherwise the real one.
        fn reporting_version(version: &str) -> Self {
            Self::new(&format!(
                "if [ \"$1\" = \"--version\" ]; then printf '%s\\n' {}; exit 0; fi",
                shell_single_quote(version)
            ))
        }

        fn path(&self) -> &Path {
            &self.dir
        }
    }

    impl Drop for FakeGit {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const MERGE_AUTHOR: &str = "Merge Author <merge-author@example.com>";
    const MERGE_AUTHOR_DATE: &str = "2024-01-02T03:04:05+00:00";

    /// The branch under test, built to the point just before the rebase.
    ///
    /// ```text
    /// main:    A --- M1 ------------------ M2   (origin/main = M2)
    ///           \      \
    /// feature:   B1 --- merge(main) --- B2
    /// ```
    ///
    /// `A` has `base.txt`, `B1` adds `feature.txt`, `M1` adds `main-only.txt`.
    /// The merge is authored by [`MERGE_AUTHOR`] at [`MERGE_AUTHOR_DATE`];
    /// `edit_merge` runs after the automatic merge and before the merge commit,
    /// so whatever it writes becomes that merge's hand edits. `later` writes
    /// `B2`, and `advance` writes `M2`.
    struct Fixture {
        repo: TempGitRepo,
        merge_sha: String,
        merge_short: String,
    }

    fn fixture(
        edit_merge: impl FnOnce(&TempGitRepo),
        later: impl FnOnce(&TempGitRepo),
        advance: impl FnOnce(&TempGitRepo),
    ) -> Fixture {
        let repo = TempGitRepo::new();
        repo.write_file("base.txt", "base\n");
        repo.commit("chore: base");

        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("feature.txt", "feature\n");
        repo.commit("feat: add feature");

        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-only.txt", "main only\n");
        repo.commit("chore: main only");

        repo.run_git(&["checkout", "feature"]);
        repo.run_git(&["merge", "--no-commit", "--no-ff", "main"]);
        edit_merge(&repo);
        repo.run_git(&["add", "-A"]);
        repo.run_git(&[
            "commit",
            "-m",
            "Merge main into feature",
            "--author",
            MERGE_AUTHOR,
            "--date",
            MERGE_AUTHOR_DATE,
        ]);
        let merge_sha = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
        let merge_short = repo
            .run_git(&["rev-parse", "--short", "HEAD"])
            .trim()
            .to_string();

        later(&repo);
        repo.run_git(&["add", "-A"]);
        repo.run_git(&["commit", "-m", "feat: later work"]);

        repo.run_git(&["checkout", "main"]);
        advance(&repo);
        repo.run_git(&["add", "-A"]);
        repo.run_git(&["commit", "-m", "chore: advance main"]);
        let main_tip = repo.run_git(&["rev-parse", "HEAD"]).trim().to_string();
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &main_tip]);
        repo.run_git(&["checkout", "feature"]);

        Fixture {
            repo,
            merge_sha,
            merge_short,
        }
    }

    /// The fixture whose hand edits create `hand-edit.txt`, which `B2` then
    /// extends. This is the failure from the branch note.
    fn hand_edit_fixture() -> Fixture {
        fixture(
            |repo| repo.write_file("hand-edit.txt", "hand edit\n"),
            |repo| repo.write_file("hand-edit.txt", "hand edit\nlater line\n"),
            |repo| repo.write_file("main-later.txt", "main later\n"),
        )
    }

    /// A branch with a single commit on top of main and no merges.
    fn merge_free_repo() -> TempGitRepo {
        let repo = TempGitRepo::new();
        repo.write_file("base.txt", "base\n");
        repo.commit("chore: base");
        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("feature.txt", "feature\n");
        repo.commit("feat: add feature");
        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-later.txt", "main later\n");
        let main_tip = repo.commit("chore: advance main");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &main_tip]);
        repo.run_git(&["checkout", "feature"]);
        repo
    }

    fn subjects(repo: &TempGitRepo, range: &str) -> Vec<String> {
        repo.run_git(&["log", "--reverse", "--format=%s", range])
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn show(repo: &TempGitRepo, rev: &str, format: &str) -> String {
        repo.run_git(&["log", "-1", &format!("--format={format}"), rev])
            .trim_end_matches('\n')
            .to_string()
    }

    fn file_at_head(repo: &TempGitRepo, path: &str) -> String {
        repo.run_git(&["show", &format!("HEAD:{path}")])
    }

    fn rev(repo: &TempGitRepo, name: &str) -> String {
        repo.run_git(&["rev-parse", name]).trim().to_string()
    }

    /// The fixture whose hand edits nothing later builds on: `B2` touches only
    /// `feature.txt`. This is the shape in which losing the hand edits is
    /// silent. A plain rebase of this branch completes without a conflict and
    /// `hand-edit.txt` is simply gone.
    fn independent_hand_edit_fixture() -> Fixture {
        fixture(
            |repo| repo.write_file("hand-edit.txt", "hand edit\n"),
            |repo| repo.write_file("feature.txt", "feature\nmore\n"),
            |repo| repo.write_file("main-later.txt", "main later\n"),
        )
    }

    /// The generated command with the statement that writes the helper file at
    /// `path` replaced by one that writes `replacement` instead, standing in
    /// for a helper that was damaged on its way to disk.
    fn command_with_damaged_helper(
        onto: &str,
        path: &str,
        original: &[&str],
        replacement: &[&str],
    ) -> String {
        let command = rebase_preserving_merge_edits_command(onto);
        let genuine = write_file_statement(path, original);
        assert!(command.contains(&genuine), "{command}");
        command.replace(&genuine, &write_file_statement(path, replacement))
    }

    fn assert_branch_untouched(repo: &TempGitRepo, original_tip: &str, context: &str) {
        assert!(
            !repo.path().join(".git/rebase-merge").exists(),
            "{context}: no rebase may be in progress"
        );
        assert!(!repo.path().join(".git/rebase-apply").exists(), "{context}");
        assert_eq!(rev(repo, "feature"), original_tip, "{context}");
        assert_eq!(rev(repo, "HEAD"), original_tip, "{context}");
        assert_eq!(
            repo.run_git(&["symbolic-ref", "HEAD"]).trim(),
            "refs/heads/feature",
            "{context}"
        );
        assert_eq!(
            repo.run_git(&["status", "--porcelain"]).trim(),
            "",
            "{context}"
        );
    }

    #[test]
    fn command_embeds_the_quoted_ref_and_avoids_rebase_merges() {
        let command = rebase_preserving_merge_edits_command("origin/main");
        assert!(command.starts_with("onto='origin/main'; "), "{command}");
        assert!(command.contains("git merge-tree --write-tree"));
        assert!(command.contains("--signoff"));
        assert!(!command.contains("--rebase-merges"));
        assert!(!command.contains("--no-update-refs"));
        assert!(!command.contains(ONTO_PLACEHOLDER));
        assert!(!command.contains(REQUIRED_GIT_PLACEHOLDER));
        assert!(!command.contains(WRITE_HELPERS_PLACEHOLDER));
        assert!(!command.contains(EDITOR_LINE_COUNT_PLACEHOLDER));
        assert!(
            command.contains(&format!("; required_git={REQUIRED_GIT_VERSION}; ")),
            "{command}"
        );
        assert!(
            command.contains(&format!(
                "[ \"$(grep -c '' \"$tmp/edit-todo\")\" -eq {} ]",
                EDITOR_LINES.len()
            )),
            "{command}"
        );
        assert!(
            command.contains(
                "> \"$tmp/edit-todo\" || fail \"cannot write the todo editor to $tmp\"; "
            ),
            "{command}"
        );
        assert!(
            command.contains(
                "> \"$tmp/splice.awk\" || fail \"cannot write the todo splice to $tmp\"; "
            ),
            "{command}"
        );

        let quoted = rebase_preserving_merge_edits_command("origin/it's");
        assert!(quoted.starts_with("onto='origin/it'\\''s'; "), "{quoted}");
    }

    /// Pipeline commands reach remote workspaces as one argv element through
    /// `sq blox ws exec … sh -c "… sh -lc '<command>'"`. Whether an embedded
    /// newline survives that cannot be verified here, so the command must not
    /// contain one, nor any other raw control character.
    #[test]
    fn command_is_a_single_line_with_no_raw_control_characters() {
        for onto in ["origin/main", "origin/it's", "origin/a b"] {
            let command = rebase_preserving_merge_edits_command(onto);
            assert!(!command.contains('\n'), "raw newline in: {command}");
            assert!(!command.contains('\r'), "raw carriage return in: {command}");
            assert!(!command.contains('\t'), "raw tab in: {command}");
            assert!(
                !command.chars().any(|c| c.is_control()),
                "control character in: {command}"
            );
        }
        for line in EDITOR_LINES.iter().chain(SPLICE_AWK_LINES) {
            assert!(
                !line.chars().any(|c| c.is_control()),
                "helper line has a raw control character: {line:?}"
            );
        }
    }

    /// The helpers written at run time must reproduce their source lines
    /// exactly, including the lines that contain single quotes, under both
    /// shells.
    #[test]
    fn helper_files_are_written_verbatim_by_printf() {
        for shell in shells() {
            let dir = std::env::temp_dir().join(format!("staged-helper-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let editor = dir.join("edit-todo");
            let awk = dir.join("splice.awk");
            let script = format!(
                "{}; {}",
                write_file_statement(&editor.to_string_lossy(), EDITOR_LINES),
                write_file_statement(&awk.to_string_lossy(), SPLICE_AWK_LINES)
            );
            let status = Command::new(shell)
                .arg("-c")
                .arg(&script)
                .status()
                .expect("shell should spawn");
            assert!(status.success(), "{shell}: {script}");
            let mut expected_editor = EDITOR_LINES.join("\n");
            expected_editor.push('\n');
            assert_eq!(
                std::fs::read_to_string(&editor).unwrap(),
                expected_editor,
                "{shell}"
            );
            let mut expected_awk = SPLICE_AWK_LINES.join("\n");
            expected_awk.push('\n');
            assert_eq!(
                std::fs::read_to_string(&awk).unwrap(),
                expected_awk,
                "{shell}"
            );
            let syntax = Command::new(shell)
                .arg("-n")
                .arg(&editor)
                .status()
                .expect("shell should spawn");
            assert!(syntax.success(), "{shell} -n rejected the editor");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    /// The failure from the branch note: a merge of main whose hand edits
    /// created content in files the automatic merge never touched, followed by
    /// a commit that builds on that content. A plain rebase drops the edits and
    /// the later commit conflicts against a file that no longer exists.
    #[test]
    fn hand_edits_in_a_merge_survive_the_rebase_in_position_with_their_author() {
        for shell in shells() {
            let fixture = hand_edit_fixture();
            let repo = &fixture.repo;

            let run = run_rebase_with(
                repo,
                "origin/main",
                RunOptions {
                    shell: Some(shell),
                    ..RunOptions::default()
                },
            );
            assert!(
                run.success,
                "{shell}: rebase should complete:\n{}",
                run.output
            );

            // Linear history with the hand edits at the merge's position.
            assert_eq!(
                subjects(repo, "origin/main..HEAD"),
                vec![
                    "feat: add feature".to_string(),
                    format!("{HAND_EDIT_COMMIT_SUBJECT_PREFIX}{}", fixture.merge_short),
                    "feat: later work".to_string(),
                ],
                "{shell}"
            );
            assert_eq!(
                repo.run_git(&["rev-list", "--merges", "origin/main..HEAD"])
                    .trim(),
                ""
            );
            assert_eq!(
                repo.run_git(&["symbolic-ref", "HEAD"]).trim(),
                "refs/heads/feature",
                "the rebase must finish on the branch, not a detached HEAD"
            );
            assert_eq!(repo.run_git(&["status", "--porcelain"]).trim(), "");

            // The content is at the tip, with the later commit applied on top.
            assert_eq!(
                file_at_head(repo, "hand-edit.txt"),
                "hand edit\nlater line\n"
            );
            assert_eq!(file_at_head(repo, "main-later.txt"), "main later\n");
            assert_eq!(file_at_head(repo, "main-only.txt"), "main only\n");

            // The synthetic commit keeps the merge's author and author date, and
            // its message says where the content came from.
            assert_eq!(show(repo, "HEAD~1", "%an <%ae>"), MERGE_AUTHOR);
            assert_eq!(
                show(repo, "HEAD~1", "%at %ai"),
                show(repo, &fixture.merge_sha, "%at %ai"),
                "the synthetic commit must keep the merge's author date"
            );
            let body = show(repo, "HEAD~1", "%B");
            assert!(body.contains(&fixture.merge_sha), "{body}");
            assert!(body.contains("hand edits"), "{body}");
            assert!(body.contains("hand-edit.txt"), "{body}");
            assert!(body.contains("Signed-off-by:"), "{body}");
            assert_eq!(
                repo.run_git(&["diff-tree", "-r", "--name-only", "--no-commit-id", "HEAD~1"])
                    .trim(),
                "hand-edit.txt",
                "the synthetic commit must contain only the hand edits"
            );

            // The summary names the merge, the files, and the outcome.
            assert!(run.output.contains(&fixture.merge_short), "{}", run.output);
            assert!(
                run.output
                    .contains("hand edits kept as a separate commit by Merge Author"),
                "{}",
                run.output
            );
            assert!(run.output.contains("      hand-edit.txt"), "{}", run.output);
            assert!(
                run.output.contains(
                    "Rebase complete: 1 merge commit(s) linearised, 0 pure automatic merge(s) dropped."
                ),
                "{}",
                run.output
            );
            assert!(
                run.output.contains("Commits on the rebased branch:"),
                "{}",
                run.output
            );
            assert!(
                !run.output.contains("note: cannot parse the git version"),
                "{}",
                run.output
            );
        }
    }

    /// With `rebase.updateRefs` on, git writes `update-ref refs/heads/<x>`
    /// directly after the pick a stacked branch points at. The hand-edit pick
    /// must go after that line, or the stacked branch ends up on the hand-edit
    /// commit instead of the rebased copy of its own commit.
    #[test]
    fn update_refs_sibling_branch_follows_its_own_commit_not_the_hand_edits() {
        for shell in shells() {
            let fixture = hand_edit_fixture();
            let repo = &fixture.repo;
            // `feature~2` walks first parents: B2, the merge, then B1.
            let first = rev(repo, "feature~2");
            assert_eq!(show(repo, &first, "%s"), "feat: add feature");
            repo.run_git(&["branch", "other", &first]);
            repo.run_git(&["config", "rebase.updateRefs", "true"]);

            let run = run_rebase_with(
                repo,
                "origin/main",
                RunOptions {
                    shell: Some(shell),
                    ..RunOptions::default()
                },
            );
            assert!(run.success, "{shell}: {}", run.output);
            assert!(
                run.output.contains("refs/heads/other"),
                "{shell}: git should report updating the stacked ref:\n{}",
                run.output
            );

            assert_eq!(
                subjects(repo, "origin/main..HEAD"),
                vec![
                    "feat: add feature".to_string(),
                    format!("{HAND_EDIT_COMMIT_SUBJECT_PREFIX}{}", fixture.merge_short),
                    "feat: later work".to_string(),
                ],
                "{shell}"
            );
            let other = rev(repo, "other");
            assert_ne!(
                other, first,
                "{shell}: other must have been moved by the rebase"
            );
            assert_eq!(
                show(repo, "other", "%s"),
                "feat: add feature",
                "{shell}: other must point at the rebased copy of its own commit"
            );
            assert_eq!(other, rev(repo, "HEAD~2"), "{shell}");
            assert_eq!(
                file_at_head(repo, "hand-edit.txt"),
                "hand edit\nlater line\n"
            );
        }
    }

    /// The splice is the one step whose failure would be silent. An awk
    /// program that merely copies the todo hands git a valid todo without the
    /// hand-edit picks; the rebase would complete, and the summary would call
    /// each merge's edits "already present" and harmlessly dropped. The editor
    /// must instead refuse before the branch is touched and name the picks
    /// that went missing.
    #[test]
    fn splice_that_loses_the_inserts_is_refused_before_the_branch_is_touched() {
        for shell in shells() {
            let fixture = independent_hand_edit_fixture();
            let repo = &fixture.repo;
            let original_tip = rev(repo, "feature");
            let command = command_with_damaged_helper(
                "origin/main",
                "$tmp/splice.awk",
                SPLICE_AWK_LINES,
                &["{ print }"],
            );

            let run = run_rebase_with(
                repo,
                "origin/main",
                RunOptions {
                    shell: Some(shell),
                    command: Some(command),
                    ..RunOptions::default()
                },
            );
            assert!(
                !run.success,
                "{shell}: the rebase must be refused:\n{}",
                run.output
            );
            let named = run
                .output
                .lines()
                .find(|line| {
                    line.starts_with(
                        "staged-rebase: preserved pick missing from the rebase todo: pick ",
                    )
                })
                .unwrap_or_else(|| {
                    panic!("{shell}: the missing pick must be named:\n{}", run.output)
                });
            assert!(
                named.ends_with(&format!(
                    " {HAND_EDIT_COMMIT_SUBJECT_PREFIX}{}",
                    fixture.merge_short
                )),
                "{shell}: {named}"
            );
            assert!(
                run.output
                    .contains("1 of 1 preserved hand-edit pick(s) did not reach the rebase todo"),
                "{shell}: {}",
                run.output
            );
            assert!(
                run.output.contains("refusing to start the rebase"),
                "{shell}: {}",
                run.output
            );
            assert!(
                !run.output.contains("Rebase complete")
                    && !run.output.contains("were already present on"),
                "{shell}: the loss must not be reported as benign:\n{}",
                run.output
            );
            assert_branch_untouched(repo, &original_tip, shell);
            assert_eq!(file_at_head(repo, "hand-edit.txt"), "hand edit\n");
        }
    }

    /// A copy of the editor cut short at a line boundary before the splice is
    /// still valid `sh`, and it has also lost its own presence check. Git would
    /// run it, get the todo back unmodified, and complete the rebase without
    /// the hand edits. The command compares the written editor's line count
    /// with the source before the rebase starts.
    #[test]
    fn truncated_todo_editor_is_refused_before_the_branch_is_touched() {
        let awk_call = EDITOR_LINES
            .iter()
            .position(|line| line.starts_with("awk "))
            .expect("the editor calls awk");
        let truncated = &EDITOR_LINES[..awk_call];
        for shell in shells() {
            let fixture = independent_hand_edit_fixture();
            let repo = &fixture.repo;
            let original_tip = rev(repo, "feature");
            let command = command_with_damaged_helper(
                "origin/main",
                "$tmp/edit-todo",
                EDITOR_LINES,
                truncated,
            );

            let run = run_rebase_with(
                repo,
                "origin/main",
                RunOptions {
                    shell: Some(shell),
                    command: Some(command),
                    ..RunOptions::default()
                },
            );
            assert!(
                !run.success,
                "{shell}: the rebase must be refused:\n{}",
                run.output
            );
            assert!(
                !run.output.contains("failed a shell syntax check"),
                "{shell}: the truncated editor is valid sh; the line count must catch it:\n{}",
                run.output
            );
            assert!(
                run.output.contains(&format!(
                    "the generated todo editor is incomplete ({} of {} lines); refusing to start the rebase",
                    truncated.len(),
                    EDITOR_LINES.len()
                )),
                "{shell}: {}",
                run.output
            );
            assert!(
                !run.output.contains("Rebase complete")
                    && !run.output.contains("were already present on"),
                "{shell}: {}",
                run.output
            );
            assert_branch_untouched(repo, &original_tip, shell);
            assert_eq!(file_at_head(repo, "hand-edit.txt"), "hand edit\n");
        }
    }

    /// A branch rebased this way once carries its earlier `Preserve hand edits
    /// from merge …` commit as an ordinary pick. When a later merge of main
    /// brings new hand edits and the branch is rebased again, the editor's
    /// presence check must look for the lines it inserted this time, not count
    /// picks whose subject looks like a hand-edit commit, or the earlier commit
    /// would make it refuse a perfectly good rebase.
    #[test]
    fn second_rebase_keeps_the_earlier_preserved_commit_and_adds_the_new_one() {
        let fixture = hand_edit_fixture();
        let repo = &fixture.repo;
        let first = run_rebase(repo, "origin/main");
        assert!(first.success, "{}", first.output);

        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-2.txt", "main 2\n");
        repo.commit("chore: main 2");
        repo.run_git(&["checkout", "feature"]);
        repo.run_git(&["merge", "--no-commit", "--no-ff", "main"]);
        repo.write_file("hand-edit-2.txt", "second hand edit\n");
        repo.run_git(&["add", "-A"]);
        repo.run_git(&["commit", "-m", "Merge main into feature again"]);
        let second_short = repo
            .run_git(&["rev-parse", "--short", "HEAD"])
            .trim()
            .to_string();

        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-3.txt", "main 3\n");
        let main_tip = repo.commit("chore: main 3");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &main_tip]);
        repo.run_git(&["checkout", "feature"]);

        let second = run_rebase(repo, "origin/main");
        assert!(second.success, "{}", second.output);
        assert!(
            !second.output.contains("preserved pick missing"),
            "{}",
            second.output
        );
        assert_eq!(
            subjects(repo, "origin/main..HEAD"),
            vec![
                "feat: add feature".to_string(),
                format!("{HAND_EDIT_COMMIT_SUBJECT_PREFIX}{}", fixture.merge_short),
                "feat: later work".to_string(),
                format!("{HAND_EDIT_COMMIT_SUBJECT_PREFIX}{second_short}"),
            ]
        );
        assert_eq!(
            file_at_head(repo, "hand-edit.txt"),
            "hand edit\nlater line\n"
        );
        assert_eq!(file_at_head(repo, "hand-edit-2.txt"), "second hand edit\n");
        assert_eq!(file_at_head(repo, "main-3.txt"), "main 3\n");
    }

    /// Main later rewrote the very lines the merge's hand edits changed. The
    /// preserved commit cannot apply cleanly, so the rebase must stop there and
    /// say which merge the content came from, never finish without it.
    #[test]
    fn conflicting_hand_edits_stop_the_rebase_and_name_the_merge() {
        let fixture = fixture(
            |repo| repo.write_file("base.txt", "base edited in the merge\n"),
            |repo| repo.write_file("feature.txt", "feature\nmore\n"),
            |repo| repo.write_file("base.txt", "base changed on main\n"),
        );
        let repo = &fixture.repo;
        let original_tip = rev(repo, "feature");

        let run = run_rebase(repo, "origin/main");
        assert!(!run.success, "the rebase must stop:\n{}", run.output);
        assert!(
            run.output.contains(&format!(
                "The rebase stopped while applying the hand edits carried by merge {}",
                fixture.merge_short
            )),
            "{}",
            run.output
        );
        assert!(
            run.output
                .contains("continue the rebase with git rebase --continue"),
            "{}",
            run.output
        );

        assert!(
            repo.path().join(".git/rebase-merge").exists(),
            "the rebase must still be in progress"
        );
        assert_eq!(
            show(repo, "REBASE_HEAD", "%s"),
            format!("{HAND_EDIT_COMMIT_SUBJECT_PREFIX}{}", fixture.merge_short)
        );
        assert_eq!(
            repo.run_git(&["diff", "--name-only", "--diff-filter=U"])
                .trim(),
            "base.txt"
        );
        assert!(
            !subjects(repo, "origin/main..HEAD").contains(&"feat: later work".to_string()),
            "commits after the merge must not be applied past the conflict"
        );

        // Nothing was lost: aborting returns the untouched branch.
        repo.run_git(&["rebase", "--abort"]);
        assert_eq!(rev(repo, "feature"), original_tip);
        assert_eq!(file_at_head(repo, "base.txt"), "base edited in the merge\n");
    }

    /// A merge that is exactly its automatic merge carries nothing of its own,
    /// so it is dropped as before and no synthetic commit appears.
    #[test]
    fn pure_automatic_merge_is_dropped_without_a_synthetic_commit() {
        let fixture = fixture(
            |_| {},
            |repo| repo.write_file("feature.txt", "feature\nmore\n"),
            |repo| repo.write_file("main-later.txt", "main later\n"),
        );
        let repo = &fixture.repo;

        let run = run_rebase(repo, "origin/main");
        assert!(run.success, "{}", run.output);
        assert_eq!(
            subjects(repo, "origin/main..HEAD"),
            vec![
                "feat: add feature".to_string(),
                "feat: later work".to_string()
            ]
        );
        assert!(!repo
            .run_git(&["log", "--format=%s", "origin/main..HEAD"])
            .contains(HAND_EDIT_COMMIT_SUBJECT_PREFIX));
        assert!(
            run.output.contains(&format!(
                "{} Merge main into feature\n    pure automatic merge: dropped",
                fixture.merge_short
            )),
            "{}",
            run.output
        );
        assert!(
            run.output.contains(
                "Rebase complete: 0 merge commit(s) linearised, 1 pure automatic merge(s) dropped."
            ),
            "{}",
            run.output
        );
        assert_eq!(file_at_head(repo, "main-only.txt"), "main only\n");
    }

    /// A merge's hand edits that main has since picked up independently make
    /// the synthetic commit empty. It is dropped rather than stopping the
    /// rebase, and the summary says so.
    #[test]
    fn hand_edits_already_on_the_new_base_are_dropped_as_empty() {
        let fixture = fixture(
            |repo| repo.write_file("hand-edit.txt", "hand edit\n"),
            |repo| repo.write_file("feature.txt", "feature\nmore\n"),
            |repo| repo.write_file("hand-edit.txt", "hand edit\n"),
        );
        let repo = &fixture.repo;

        let run = run_rebase(repo, "origin/main");
        assert!(run.success, "{}", run.output);
        assert_eq!(
            subjects(repo, "origin/main..HEAD"),
            vec![
                "feat: add feature".to_string(),
                "feat: later work".to_string()
            ]
        );
        assert!(
            run.output.contains(&format!(
                "The hand edits carried by merge {} were already present on origin/main",
                fixture.merge_short
            )),
            "{}",
            run.output
        );
        assert_eq!(file_at_head(repo, "hand-edit.txt"), "hand edit\n");
    }

    /// Two merges of main, the first pure and the second with hand edits, with
    /// branch commits between and after them. The pure merge disappears, the
    /// hand edits land after the commits that preceded the second merge and
    /// before the one that followed it.
    #[test]
    fn mixed_merges_keep_the_hand_edits_at_the_second_merge_position() {
        let repo = TempGitRepo::new();
        repo.write_file("base.txt", "base\n");
        repo.commit("chore: base");

        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("feature.txt", "feature\n");
        repo.commit("feat: one");

        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-1.txt", "main 1\n");
        repo.commit("chore: main 1");
        repo.run_git(&["checkout", "feature"]);
        repo.run_git(&["merge", "--no-ff", "-m", "Merge main (pure)", "main"]);

        repo.write_file("feature.txt", "feature\ntwo\n");
        repo.commit("feat: two");

        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-2.txt", "main 2\n");
        repo.commit("chore: main 2");
        repo.run_git(&["checkout", "feature"]);
        repo.run_git(&["merge", "--no-commit", "--no-ff", "main"]);
        repo.write_file("hand-edit.txt", "hand edit\n");
        repo.run_git(&["add", "-A"]);
        repo.run_git(&["commit", "-m", "Merge main (hand edits)"]);
        let evil_short = repo
            .run_git(&["rev-parse", "--short", "HEAD"])
            .trim()
            .to_string();

        repo.write_file("hand-edit.txt", "hand edit\nthree\n");
        repo.commit("feat: three");

        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-3.txt", "main 3\n");
        let main_tip = repo.commit("chore: main 3");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &main_tip]);
        repo.run_git(&["checkout", "feature"]);

        let run = run_rebase(&repo, "origin/main");
        assert!(run.success, "{}", run.output);
        assert_eq!(
            subjects(&repo, "origin/main..HEAD"),
            vec![
                "feat: one".to_string(),
                "feat: two".to_string(),
                format!("{HAND_EDIT_COMMIT_SUBJECT_PREFIX}{evil_short}"),
                "feat: three".to_string(),
            ]
        );
        assert_eq!(file_at_head(&repo, "hand-edit.txt"), "hand edit\nthree\n");
        assert!(
            run.output.contains(
                "Rebase complete: 1 merge commit(s) linearised, 1 pure automatic merge(s) dropped."
            ),
            "{}",
            run.output
        );
    }

    /// A branch without merges takes the plain path and behaves as before.
    #[test]
    fn branch_without_merges_rebases_plainly() {
        let repo = merge_free_repo();

        let run = run_rebase(&repo, "origin/main");
        assert!(run.success, "{}", run.output);
        assert!(
            run.output
                .contains("Merge commits in origin/main..HEAD: none"),
            "{}",
            run.output
        );
        assert_eq!(
            subjects(&repo, "origin/main..HEAD"),
            vec!["feat: add feature".to_string()]
        );
        assert!(show(&repo, "HEAD", "%B").contains("Signed-off-by:"));
        assert_eq!(file_at_head(&repo, "main-later.txt"), "main later\n");
    }

    /// A git older than 2.38 has no `merge-tree --write-tree`, so it cannot
    /// tell which merges carry hand edits. The command must refuse before the
    /// rebase starts, naming both versions and the reason, and leave the branch
    /// exactly as it was.
    #[test]
    fn old_git_is_refused_before_the_branch_is_touched_when_merges_exist() {
        let fake = FakeGit::reporting_version("git version 2.34.1");
        let fixture = hand_edit_fixture();
        let repo = &fixture.repo;
        let original_tip = rev(repo, "feature");

        let run = run_rebase_with(
            repo,
            "origin/main",
            RunOptions {
                path_prefix: Some(fake.path()),
                ..RunOptions::default()
            },
        );
        assert!(!run.success, "{}", run.output);
        assert!(
            run.output
                .contains("refusing to rebase with git version 2.34.1"),
            "{}",
            run.output
        );
        assert!(
            run.output.contains("git 2.38 or newer is required"),
            "{}",
            run.output
        );
        assert!(
            run.output
                .contains("origin/main..HEAD has 1 merge commit(s)"),
            "{}",
            run.output
        );
        assert!(
            run.output
                .contains("could drop content that exists only inside those merge commits"),
            "{}",
            run.output
        );
        assert!(
            run.output.contains("The branch was left untouched"),
            "{}",
            run.output
        );
        assert!(
            !run.output.contains("merge-tree --write-tree exited"),
            "the refusal must come from the version check, not from running merge-tree:\n{}",
            run.output
        );

        assert!(!repo.path().join(".git/rebase-merge").exists());
        assert!(!repo.path().join(".git/rebase-apply").exists());
        assert_eq!(rev(repo, "feature"), original_tip);
        assert_eq!(rev(repo, "HEAD"), original_tip);
        assert_eq!(repo.run_git(&["status", "--porcelain"]).trim(), "");
    }

    /// The version gate only matters when there are merges to inspect. A
    /// merge-free branch keeps rebasing on any git.
    #[test]
    fn old_git_still_rebases_a_branch_without_merges() {
        let fake = FakeGit::reporting_version("git version 2.34.1");
        let repo = merge_free_repo();

        let run = run_rebase_with(
            &repo,
            "origin/main",
            RunOptions {
                path_prefix: Some(fake.path()),
                ..RunOptions::default()
            },
        );
        assert!(run.success, "{}", run.output);
        assert!(
            run.output
                .contains("Merge commits in origin/main..HEAD: none"),
            "{}",
            run.output
        );
        assert_eq!(
            subjects(&repo, "origin/main..HEAD"),
            vec!["feat: add feature".to_string()]
        );
    }

    /// Versions at and above the floor pass the gate, including the floor
    /// itself with and without a patch component, a major bump and Apple's
    /// suffixed format.
    #[test]
    fn git_at_or_above_the_required_version_passes_the_gate() {
        for version in [
            "git version 2.38",
            "git version 2.38.0",
            "git version 2.39.5 (Apple Git-154)",
            "git version 3.0.0",
        ] {
            let fake = FakeGit::reporting_version(version);
            let fixture = hand_edit_fixture();
            let run = run_rebase_with(
                &fixture.repo,
                "origin/main",
                RunOptions {
                    path_prefix: Some(fake.path()),
                    ..RunOptions::default()
                },
            );
            assert!(run.success, "{version}: {}", run.output);
            assert!(
                !run.output.contains("refusing to rebase"),
                "{version}: {}",
                run.output
            );
            assert!(
                !run.output.contains("note: cannot parse"),
                "{version}: {}",
                run.output
            );
        }
    }

    /// A version string the command cannot parse is not treated as too old:
    /// the command says so and carries on, because a git that really lacks
    /// `merge-tree --write-tree` still fails loudly on its own error.
    #[test]
    fn unparseable_git_version_is_noted_and_allowed_through() {
        let fake = FakeGit::reporting_version("git version custom-build");
        let fixture = hand_edit_fixture();
        let run = run_rebase_with(
            &fixture.repo,
            "origin/main",
            RunOptions {
                path_prefix: Some(fake.path()),
                ..RunOptions::default()
            },
        );
        assert!(run.success, "{}", run.output);
        assert!(
            run.output.contains(
                "note: cannot parse the git version from \"git version custom-build\"; assuming git 2.38 or newer"
            ),
            "{}",
            run.output
        );
    }

    /// A merge of an unrelated history has no merge base, so `merge-tree`
    /// refuses with exit 128 rather than the conflict exit 1. That is not a
    /// version problem and must not be reported as one: the headline says what
    /// failed, for which merge, and git's own stderr follows.
    #[test]
    fn other_merge_tree_failures_surface_their_own_stderr_not_a_version_complaint() {
        let repo = TempGitRepo::new();
        repo.write_file("base.txt", "base\n");
        repo.commit("chore: base");
        repo.run_git(&["checkout", "-b", "feature"]);
        repo.write_file("feature.txt", "feature\n");
        repo.commit("feat: add feature");

        repo.run_git(&["checkout", "--orphan", "unrelated"]);
        repo.run_git(&["rm", "-rf", "--quiet", "."]);
        repo.write_file("unrelated.txt", "unrelated\n");
        repo.commit("chore: unrelated root");

        repo.run_git(&["checkout", "feature"]);
        repo.run_git(&[
            "merge",
            "--no-ff",
            "--allow-unrelated-histories",
            "-m",
            "Merge unrelated",
            "unrelated",
        ]);
        let merge_short = repo
            .run_git(&["rev-parse", "--short", "HEAD"])
            .trim()
            .to_string();
        let original_tip = rev(&repo, "feature");

        repo.run_git(&["checkout", "main"]);
        repo.write_file("main-later.txt", "main later\n");
        let main_tip = repo.commit("chore: advance main");
        repo.run_git(&["update-ref", "refs/remotes/origin/main", &main_tip]);
        repo.run_git(&["checkout", "feature"]);

        let run = run_rebase(&repo, "origin/main");
        assert!(!run.success, "{}", run.output);
        assert!(
            run.output.contains(&format!(
                "cannot compute the automatic merge of the parents of merge {merge_short} (git merge-tree --write-tree exited 128)"
            )),
            "{}",
            run.output
        );
        assert!(
            run.output.contains("unrelated histories"),
            "git's own stderr must be shown:\n{}",
            run.output
        );
        assert!(
            !run.output.contains("2.38") && !run.output.contains("refusing to rebase"),
            "a merge-tree failure on a capable git must not be blamed on the version:\n{}",
            run.output
        );
        assert!(!repo.path().join(".git/rebase-merge").exists());
        assert_eq!(rev(&repo, "feature"), original_tip);
    }
}
