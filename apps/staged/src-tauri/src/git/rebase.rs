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
//! is a self-contained POSIX shell script, because pipeline steps run as shell
//! commands both locally and on remote workspaces, and the safeguard has to
//! behave identically in both places.
//!
//! What the script does, before touching the branch:
//!
//! 1. Lists the merge commits in `<onto>..HEAD`, oldest first in topological
//!    order.
//! 2. For each, computes the automatic merge of its two parents with
//!    `git merge-tree --write-tree` and compares that tree with the merge's real
//!    tree. Equal trees mean a pure automatic merge, dropped exactly as before.
//! 3. For a merge whose trees differ, records two throwaway commits with
//!    `git commit-tree`: the automatic merge result, and on top of it the
//!    merge's real tree, authored by the merge's original author at the
//!    original author date. The second commit's diff against its parent is
//!    precisely the hand edits, and its message names the merge it came from.
//! 4. Runs `git rebase --interactive` with a `GIT_SEQUENCE_EDITOR` that inserts a
//!    `pick` of each hand-edit commit right after the last surviving pick that
//!    is an ancestor of the merge, so it lands where the merge sat in the
//!    sequence and later commits that depend on it still apply.
//!
//! If a hand-edit commit does not apply cleanly the rebase stops on it like any
//! other conflict, and the script names the merge the content came from. The
//! script's output doubles as the summary shown in the pipeline step: which
//! merges were linearised and which files their edits touched, which pure
//! automatic merges were dropped, and the resulting commit list.
//!
//! `--rebase-merges` is deliberately not used. It recreates merges by
//! re-running them, so hand edits are lost anyway, and a merge whose second
//! parent is already an ancestor of the new base degenerates to a no-op.

/// Token in [`SCRIPT`] that [`rebase_preserving_merge_edits_command`] replaces
/// with the shell-quoted ref to rebase onto.
const ONTO_PLACEHOLDER: &str = "__STAGED_ONTO__";

/// Subject prefix of the commits that carry a merge's hand edits. The todo
/// editor and the post-rebase summary both key off it, and the AI handoff
/// prompt in `prs` tells the agent never to drop commits that start with it.
pub const HAND_EDIT_COMMIT_SUBJECT_PREFIX: &str = "Preserve hand edits from merge ";

/// The safeguard, as a POSIX `sh` script. See the module docs for the
/// algorithm. Everything the script prints goes to stdout (git's own output is
/// redirected there too) so the pipeline step shows one ordered transcript.
const SCRIPT: &str = r##"# Staged rebase safeguard: linearise merge commits without losing hand edits.
onto=__STAGED_ONTO__

fail() {
  printf 'staged-rebase: %s\n' "$*" >&2
  exit 1
}

git rev-parse --verify --quiet "$onto^{commit}" >/dev/null || fail "cannot resolve $onto"
git rev-parse --verify --quiet 'HEAD^{commit}' >/dev/null || fail "cannot resolve HEAD"

merges=$(git rev-list --reverse --topo-order --merges "$onto..HEAD") \
  || fail "cannot list the merge commits in $onto..HEAD"

if [ -z "$merges" ]; then
  printf 'Merge commits in %s..HEAD: none\n\n' "$onto"
  exec git rebase --signoff "$onto"
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/staged-rebase.XXXXXX") || fail "cannot create a temporary directory"
trap 'rm -rf "$tmp"' EXIT
: > "$tmp/synthetic"
linearised=0
dropped=0

printf 'Merge commits in %s..HEAD:\n' "$onto"
for merge in $merges; do
  set -- $(git rev-list --parents -n 1 "$merge")
  shift
  short=$(git rev-parse --short "$merge")
  subject=$(git log -1 --format=%s "$merge")
  [ $# -eq 2 ] || fail "merge $short has $# parents; only two-parent merges can be linearised. Rebase this branch by hand."

  auto=$(git merge-tree --write-tree "$1" "$2" 2>"$tmp/merge-tree.err")
  merge_tree_status=$?
  if [ $merge_tree_status -gt 1 ]; then
    fail "git merge-tree --write-tree failed for merge $short (git 2.38 or newer is required to rebase a branch that has merge commits): $(cat "$tmp/merge-tree.err")"
  fi
  auto_tree=$(printf '%s\n' "$auto" | sed -n 1p)
  tree=$(git rev-parse --verify "$merge^{tree}") || fail "cannot read the tree of merge $short"

  if [ "$auto_tree" = "$tree" ]; then
    dropped=$((dropped + 1))
    printf '  %s %s\n    pure automatic merge: dropped\n' "$short" "$subject"
    continue
  fi

  linearised=$((linearised + 1))
  files=$(git diff-tree -r --name-only "$auto_tree" "$tree")
  author_name=$(git log -1 --format=%an "$merge")
  author_email=$(git log -1 --format=%ae "$merge")
  author_date=$(git log -1 --format=%aD "$merge")

  # Two throwaway commits: the automatic merge result, then the merge's real
  # tree on top of it. Picking the second replays exactly the hand edits.
  scaffold=$(GIT_AUTHOR_NAME="$author_name" GIT_AUTHOR_EMAIL="$author_email" GIT_AUTHOR_DATE="$author_date" \
    git commit-tree "$auto_tree" -p "$1" -m "Automatic merge result of $short (Staged rebase scaffolding)") \
    || fail "cannot record the automatic merge result of merge $short"
  message=$(printf 'Preserve hand edits from merge %s\n\nStaged linearised merge commit %s\n("%s") while rebasing onto %s. A rebase does not replay merge\ncommits, so this commit carries the hand edits that merge introduced beyond\nthe automatic merge of its parents; without it that content would be lost.\n\nFiles:\n%s\n' \
    "$short" "$merge" "$subject" "$onto" "$(printf '%s\n' "$files" | sed 's/^/  /')")
  synthetic=$(GIT_AUTHOR_NAME="$author_name" GIT_AUTHOR_EMAIL="$author_email" GIT_AUTHOR_DATE="$author_date" \
    git commit-tree "$tree" -p "$scaffold" -m "$message") \
    || fail "cannot record the hand edits of merge $short"
  printf '%s %s %s\n' "$synthetic" "$merge" "$short" >> "$tmp/synthetic"
  git rev-list "$merge" --not "$onto" > "$tmp/ancestors.$merge" \
    || fail "cannot list the ancestors of merge $short"

  note=""
  if [ $merge_tree_status -eq 1 ]; then
    note=" (the automatic merge had conflicts; their resolution is part of the preserved edits)"
  fi
  printf '  %s %s\n    hand edits kept as a separate commit by %s%s; files:\n' "$short" "$subject" "$author_name" "$note"
  printf '%s\n' "$files" | sed 's/^/      /'
done

# Sequence editor for `git rebase -i`: insert a pick of each hand-edit commit
# right after the last surviving pick that is an ancestor of its merge.
cat > "$tmp/edit-todo" <<'STAGED_EDIT_TODO'
todo=$1
dir=$(dirname "$0")
: > "$dir/picks"
count=0
while IFS= read -r line || [ -n "$line" ]; do
  case "$line" in
    "pick "*|"p "*)
      count=$((count + 1))
      set -- $line
      full=$(git rev-parse --verify --quiet "$2^{commit}" </dev/null) || exit 1
      printf '%s %s\n' "$count" "$full" >> "$dir/picks"
      ;;
  esac
done < "$todo"
: > "$dir/inserts"
while read -r synthetic merge short; do
  anchor=0
  while read -r index full; do
    if grep -qFx "$full" "$dir/ancestors.$merge"; then
      anchor=$index
    fi
  done < "$dir/picks"
  printf '%s\tpick %s Preserve hand edits from merge %s\n' "$anchor" "$synthetic" "$short" >> "$dir/inserts"
done < "$dir/synthetic"
awk -v inserts="$dir/inserts" '
  BEGIN {
    while ((getline line < inserts) > 0) {
      tab = index(line, "\t")
      key = substr(line, 1, tab - 1)
      ins[key] = ins[key] substr(line, tab + 1) "\n"
    }
    if ("0" in ins) printf "%s", ins["0"]
  }
  { print }
  /^(pick|p) / { n++; if ((n "") in ins) printf "%s", ins[n ""] }
' "$todo" > "$todo.staged" || exit 1
mv "$todo.staged" "$todo"
STAGED_EDIT_TODO

printf '\n'
GIT_SEQUENCE_EDITOR="sh '$tmp/edit-todo'" \
  git -c rebase.abbreviateCommands=false rebase --interactive --signoff --empty=drop --no-autosquash "$onto" 2>&1
status=$?

if [ $status -ne 0 ]; then
  stopped=$(git rev-parse --verify --quiet REBASE_HEAD 2>/dev/null)
  if [ -n "$stopped" ]; then
    stopped_on=$(grep "^$stopped " "$tmp/synthetic" | cut -d' ' -f3)
    if [ -n "$stopped_on" ]; then
      printf '\nThe rebase stopped while applying the hand edits carried by merge %s. That content exists in no other commit: resolve the conflict so it is kept, then continue the rebase. Do not drop this commit.\n' "$stopped_on"
    fi
  fi
  exit $status
fi

printf '\nRebase complete: %s merge commit(s) linearised, %s pure automatic merge(s) dropped.\n' "$linearised" "$dropped"
while read -r synthetic merge short; do
  if ! git log --format=%s "$onto..HEAD" | grep -qFx "Preserve hand edits from merge $short"; then
    printf 'The hand edits carried by merge %s were already present on %s; their commit became empty and was dropped.\n' "$short" "$onto"
  fi
done < "$tmp/synthetic"
printf '\nCommits on the rebased branch:\n'
git log --reverse --format='  %h %s' "$onto..HEAD"
"##;

/// The shell command that rebases the current branch onto `onto_ref` (for
/// example `origin/main`) while preserving hand edits carried by merge commits.
///
/// Signs off every replayed commit, as Staged's rebase always has. Prints a
/// summary of what happened to each merge commit, exits non-zero when the
/// rebase stops (on a conflict or otherwise), and names the merge a conflicting
/// hand-edit commit came from. See the module docs for the full behaviour.
pub fn rebase_preserving_merge_edits_command(onto_ref: &str) -> String {
    SCRIPT.replace(ONTO_PLACEHOLDER, &shell_single_quote(onto_ref))
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
    use std::process::Command;

    /// What the rebase command produced: the pipeline sees the same two things,
    /// the exit status and the combined output.
    struct RebaseRun {
        success: bool,
        output: String,
    }

    /// Run the generated command the way a pipeline step does, `sh -c` in the
    /// worktree, isolated from the developer's own git config.
    fn run_rebase(repo: &TempGitRepo, onto: &str) -> RebaseRun {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(rebase_preserving_merge_edits_command(onto))
            .current_dir(repo.path());
        strip_git_env(&mut command);
        command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        let output = command.output().expect("sh should spawn");
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        RebaseRun {
            success: output.status.success(),
            output: text,
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

    #[test]
    fn command_embeds_the_quoted_ref_and_avoids_rebase_merges() {
        let command = rebase_preserving_merge_edits_command("origin/main");
        assert!(command.contains("onto='origin/main'"), "{command}");
        assert!(command.contains("git merge-tree --write-tree"));
        assert!(command.contains("--signoff"));
        assert!(!command.contains("--rebase-merges"));
        assert!(!command.contains(ONTO_PLACEHOLDER));

        let quoted = rebase_preserving_merge_edits_command("origin/it's");
        assert!(quoted.contains("onto='origin/it'\\''s'"), "{quoted}");
    }

    /// The failure from the branch note: a merge of main whose hand edits
    /// created content in files the automatic merge never touched, followed by
    /// a commit that builds on that content. A plain rebase drops the edits and
    /// the later commit conflicts against a file that no longer exists.
    #[test]
    fn hand_edits_in_a_merge_survive_the_rebase_in_position_with_their_author() {
        let fixture = fixture(
            |repo| repo.write_file("hand-edit.txt", "hand edit\n"),
            |repo| repo.write_file("hand-edit.txt", "hand edit\nlater line\n"),
            |repo| repo.write_file("main-later.txt", "main later\n"),
        );
        let repo = &fixture.repo;

        let run = run_rebase(repo, "origin/main");
        assert!(run.success, "rebase should complete:\n{}", run.output);

        // Linear history with the hand edits at the merge's position.
        assert_eq!(
            subjects(repo, "origin/main..HEAD"),
            vec![
                "feat: add feature".to_string(),
                format!("{HAND_EDIT_COMMIT_SUBJECT_PREFIX}{}", fixture.merge_short),
                "feat: later work".to_string(),
            ]
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
        let original_tip = repo.run_git(&["rev-parse", "feature"]).trim().to_string();

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
        assert_eq!(repo.run_git(&["rev-parse", "feature"]).trim(), original_tip);
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
}
