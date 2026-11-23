//! The create-PR pipeline steps.

use super::*;

#[test]
fn create_pr_pipeline_fetches_base_and_uses_origin_merge_base_for_context() {
    let steps = build_create_pr_pipeline_steps(
        "pull request",
        "main",
        "",
        "feature-branch-2",
        "feature-branch",
    );

    assert_eq!(steps.len(), 5);

    let (label, command, on_failure) = command_at(&steps, 0);
    assert_eq!(label, "Fetch latest base");
    assert_eq!(
        command,
        "if ! git fetch origin main; then git -c 'url.https://github.com/.insteadOf=git@github.com:' fetch origin main; fi"
    );
    assert!(matches!(on_failure, FailureStrategy::HandoffToAi { .. }));

    let (_, command, _) = command_at(&steps, 1);
    assert_eq!(
        command,
        r#"base_commit=$(git merge-base origin/main HEAD) && git log --oneline "$base_commit"..HEAD"#
    );
    assert!(!command.contains("git log --oneline main"));

    let (_, command, _) = command_at(&steps, 2);
    assert_eq!(
        command,
        r#"base_commit=$(git merge-base origin/main HEAD) && git diff "$base_commit"..HEAD --stat"#
    );
    assert!(!command.contains("git diff main"));

    let (label, command, _) = command_at(&steps, 3);
    assert_eq!(label, "Push to remote");
    assert_eq!(
        command,
        "if ! git push -u origin feature-branch-2:feature-branch; then git -c 'url.https://github.com/.insteadOf=git@github.com:' push -u origin feature-branch-2:feature-branch; fi"
    );

    let (label, prompt) = ai_prompt_at(&steps, 4);
    assert_eq!(label, "Create PR");
    assert!(prompt.contains("local `main` branch may be stale"));
    assert!(prompt.contains("$(git merge-base origin/main HEAD)..HEAD"));
    assert!(prompt.contains("gh pr create --base main --head feature-branch "));
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
