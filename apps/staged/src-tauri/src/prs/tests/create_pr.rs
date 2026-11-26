//! The create-PR pipeline steps.

use super::*;
use crate::prs::create_pr::CHECK_LOCAL_BRANCH_LABEL;
use crate::test_utils::{run_git_in, TempDir, TempGitRepo};

#[test]
fn create_pr_pipeline_fetches_base_and_uses_origin_merge_base_for_context() {
    let steps = build_create_pr_pipeline_steps(
        "pull request",
        "main",
        "",
        "feature-branch-2",
        "feature-branch",
    );

    assert_eq!(steps.len(), 6);

    let (label, command, on_failure) = command_at(&steps, 0);
    assert_eq!(label, CHECK_LOCAL_BRANCH_LABEL);
    assert!(
        command.starts_with("if git show-ref --verify --quiet refs/heads/feature-branch-2; then"),
        "{command}"
    );
    assert!(matches!(
        on_failure,
        FailureStrategy::Abort { marker: None }
    ));

    let (label, command, on_failure) = command_at(&steps, 1);
    assert_eq!(label, "Fetch latest base");
    assert_eq!(
        command,
        "if ! git fetch origin main; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin main; fi"
    );
    let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
        panic!("the fetch hands off: {on_failure:?}");
    };
    assert!(prompt_template.contains(
        "inspect `feature-branch-2` against `origin/main`, push it with exactly \
         `git push -u origin feature-branch-2:feature-branch`"
    ));

    let (_, command, _) = command_at(&steps, 2);
    assert_eq!(
        command,
        r#"base_commit=$(git merge-base origin/main feature-branch-2) && git log --oneline "$base_commit"..feature-branch-2"#
    );
    assert!(!command.contains("git log --oneline main"));

    let (_, command, _) = command_at(&steps, 3);
    assert_eq!(
        command,
        r#"base_commit=$(git merge-base origin/main feature-branch-2) && git diff "$base_commit"..feature-branch-2 --stat"#
    );
    assert!(!command.contains("git diff main"));

    let (label, command, _) = command_at(&steps, 4);
    assert_eq!(label, "Push to remote");
    assert_eq!(
        command,
        "if ! git push -u origin feature-branch-2:feature-branch; then if git ls-remote --upload-pack=git-receive-pack origin HEAD >/dev/null 2>&1; then false; else git -c 'url.https://github.com/.insteadOf=git@github.com:' push -u origin feature-branch-2:feature-branch; fi; fi"
    );

    let (label, prompt) = ai_prompt_at(&steps, 5);
    assert_eq!(label, "Create PR");
    assert!(prompt.contains("Create a pull request for the local branch `feature-branch-2`."));
    assert!(prompt.contains("local `main` branch may be stale"));
    assert!(prompt.contains("$(git merge-base origin/main feature-branch-2)..feature-branch-2"));
    assert!(prompt.contains("gh pr create --base main --head feature-branch "));
    assert!(prompt.contains("`gh pr view feature-branch --json url --jq .url`"));
    assert!(prompt.contains("`gh pr edit feature-branch`"));
}

/// Every command a step runs and every prompt an agent could be handed.
fn commands_and_prompts(steps: &[PipelineStep]) -> Vec<&str> {
    let mut texts = Vec::new();
    for step in steps {
        match step {
            PipelineStep::Command {
                command,
                on_failure,
                ..
            } => {
                texts.push(command.as_str());
                if let FailureStrategy::HandoffToAi { prompt_template } = on_failure {
                    texts.push(prompt_template.as_str());
                }
            }
            PipelineStep::AiHandoff {
                prompt_template, ..
            } => texts.push(prompt_template.as_str()),
        }
    }
    texts
}

/// Run a step's command in `dir` the way the pipeline does, with `sh` and
/// with `dash` where the machine has it (a workstation's `/bin/sh`), and
/// return its exit code and what it printed.
fn run_step(dir: &std::path::Path, command: &str) -> Vec<(Option<i32>, String)> {
    let mut shells = vec!["sh"];
    if std::path::Path::new("/bin/dash").exists() {
        shells.push("/bin/dash");
    }
    shells
        .into_iter()
        .map(|shell| {
            let mut sh = std::process::Command::new(shell);
            sh.arg("-c").arg(command).current_dir(dir);
            crate::git::strip_git_env(&mut sh);
            let output = sh.output().unwrap();
            let printed = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            (output.status.code(), printed)
        })
        .collect()
}

/// A clone of a bare origin, with `feature-b` one commit ahead of
/// `origin/main` and not pushed.
fn clone_with_feature_b() -> (TempDir, TempDir) {
    let seed = TempGitRepo::new();
    seed.write_file("file.txt", "base\n");
    seed.commit("base");
    let origin = TempDir::new("origin");
    let origin_path = origin.path().to_str().unwrap();
    run_git_in(seed.path(), &["clone", "--bare", ".", origin_path]);
    let clone = TempDir::new("workstation");
    run_git_in(
        origin.path(),
        &["clone", origin_path, clone.path().to_str().unwrap()],
    );
    run_git_in(clone.path(), &["config", "user.email", "test@example.com"]);
    run_git_in(clone.path(), &["config", "user.name", "Test"]);
    run_git_in(clone.path(), &["checkout", "-b", "feature-b"]);
    std::fs::write(clone.path().join("b.txt"), "b\n").unwrap();
    run_git_in(clone.path(), &["add", "b.txt"]);
    run_git_in(clone.path(), &["commit", "-m", "feature-b's work"]);
    (origin, clone)
}

/// The pipeline describes its own branch whatever the clone has checked
/// out. Here that is a sibling branch with work of its own, as it is while
/// a sibling's session has a shared clone: the history and changed-files
/// steps used to read HEAD, and the agent was told to compare against HEAD,
/// so the pull request for `feature-b` was written from `feature-a`'s
/// commits. No command and no prompt reads HEAD now, and the prompt tells
/// the agent to leave the checkout where it is.
#[test]
fn the_pull_request_describes_its_branch_from_a_clone_on_a_sibling_branch() {
    let (_origin, clone) = clone_with_feature_b();
    run_git_in(
        clone.path(),
        &["checkout", "-b", "feature-a", "origin/main"],
    );
    std::fs::write(clone.path().join("a.txt"), "a\n").unwrap();
    run_git_in(clone.path(), &["add", "a.txt"]);
    run_git_in(clone.path(), &["commit", "-m", "feature-a's work"]);

    let steps =
        build_create_pr_pipeline_steps("pull request", "main", "", "feature-b", "feature-b");
    for text in commands_and_prompts(&steps) {
        assert!(
            !text.contains("HEAD)") && !text.contains("..HEAD"),
            "{text}"
        );
        assert!(!text.contains("the current branch"), "{text}");
    }
    let (_, prompt) = ai_prompt_at(&steps, 5);
    assert!(
        prompt.contains("do not check out, switch, create or reset branches"),
        "{prompt}"
    );

    for (code, printed) in run_step(clone.path(), command_at(&steps, 0).1) {
        assert_eq!(code, Some(0), "{printed}");
    }
    for (code, history) in run_step(clone.path(), command_at(&steps, 2).1) {
        assert_eq!(code, Some(0), "{history}");
        assert!(history.contains("feature-b's work"), "{history}");
        assert!(!history.contains("feature-a's work"), "{history}");
    }
    for (code, changed) in run_step(clone.path(), command_at(&steps, 3).1) {
        assert_eq!(code, Some(0), "{changed}");
        assert!(changed.contains("b.txt"), "{changed}");
        assert!(!changed.contains("a.txt"), "{changed}");
    }
    assert_eq!(
        run_git_in(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
        "feature-a"
    );
}

/// The state a refused or blocked setup leaves: the clone on `main` with
/// the agent's commits and no local branch. The push would fail there and
/// hand an agent "fix it, then retry the push", which it does by creating
/// the branch setup would not. The first step stops the pipeline instead,
/// before any step that hands off, with words for the user and nothing
/// changed. A check that could not run (no repository) is not reported as a
/// missing branch, and stops the pipeline all the same.
#[test]
fn a_clone_without_the_local_branch_stops_the_pull_request_before_any_agent() {
    let (_origin, clone) = clone_with_feature_b();
    run_git_in(clone.path(), &["checkout", "main"]);
    std::fs::write(clone.path().join("stranded.txt"), "work\n").unwrap();
    run_git_in(clone.path(), &["add", "stranded.txt"]);
    run_git_in(clone.path(), &["commit", "-m", "agent work"]);
    let before = run_git_in(
        clone.path(),
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    );

    let steps =
        build_create_pr_pipeline_steps("pull request", "main", "", "feature-new", "feature-new");
    let hands_off = |step: &PipelineStep| match step {
        PipelineStep::Command { on_failure, .. } => {
            matches!(on_failure, FailureStrategy::HandoffToAi { .. })
        }
        PipelineStep::AiHandoff { .. } => true,
    };
    assert_eq!(steps.iter().position(hands_off), Some(1));
    let (label, command, on_failure) = command_at(&steps, 0);
    assert_eq!(label, CHECK_LOCAL_BRANCH_LABEL);
    assert!(matches!(
        on_failure,
        FailureStrategy::Abort { marker: None }
    ));

    for (code, printed) in run_step(clone.path(), command) {
        assert_eq!(code, Some(1), "{printed}");
        assert!(
            printed.starts_with("Local branch 'feature-new' does not exist in this clone"),
            "{printed}"
        );
        assert!(printed.contains("no pull request was created"), "{printed}");
        assert!(printed.contains("retry setup"), "{printed}");
    }
    assert_eq!(
        run_git_in(
            clone.path(),
            &["for-each-ref", "--format=%(refname) %(objectname)"]
        ),
        before
    );
    assert_eq!(
        run_git_in(clone.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );

    let no_repository = TempDir::new("no-repository");
    for (code, printed) in run_step(no_repository.path(), command) {
        assert!(code != Some(0) && code != Some(1), "{code:?}: {printed}");
        assert!(
            !printed.contains("does not exist in this clone"),
            "{printed}"
        );
    }
}

/// Every request the create-PR pipeline hands an agent is built by
/// `create_pr_handoff_prompt`: the hand-off that creates the pull request,
/// and the ones that take over when the fetch or the push fails.
#[test]
fn create_pr_handoff_prompts_open_with_instructions_the_title_check_recognises() {
    for (pr_type, draft_flag) in [("pull request", ""), ("draft pull request", " --draft")] {
        let steps = build_create_pr_pipeline_steps(
            pr_type,
            "main",
            draft_flag,
            "feature-branch-2",
            "feature-branch",
        );
        let templates: Vec<&str> = steps
            .iter()
            .filter_map(|step| match step {
                PipelineStep::AiHandoff {
                    prompt_template, ..
                }
                | PipelineStep::Command {
                    on_failure: FailureStrategy::HandoffToAi { prompt_template },
                    ..
                } => Some(prompt_template.as_str()),
                PipelineStep::Command { .. } => None,
            })
            .collect();

        assert_eq!(templates.len(), 3, "{pr_type}");
        for template in templates {
            assert_title_check_recognises_instructions(template);
        }
    }
}
