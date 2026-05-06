//! Revert: the pipeline step, its handoff prompt and SHA validation.

use super::*;

#[test]
fn revert_pipeline_uses_signoff_and_hands_off_to_ai() {
    let steps = build_revert_pipeline_steps("abc1234").unwrap();

    assert_eq!(steps.len(), 1);
    let (label, command, on_failure) = command_at(&steps, 0);
    assert_eq!(label, "Revert commit");
    assert_eq!(command, "git revert --signoff --no-edit abc1234");

    let FailureStrategy::HandoffToAi { prompt_template } = on_failure else {
        panic!("expected revert failure to hand off to AI");
    };
    assert!(prompt_template.contains("git revert --continue"));
    assert!(prompt_template.contains("preserving unrelated local changes"));
    assert!(prompt_template.contains("Do not push the branch"));
    assert!(prompt_template.contains("{step_outputs}"));
}

#[test]
fn revert_pipeline_rejects_unsafe_commit_sha() {
    assert!(build_revert_pipeline_steps("abc1234").is_ok());
    assert!(build_revert_pipeline_steps("abcdef").is_err());
    assert!(build_revert_pipeline_steps("abc1234; rm -rf /").is_err());
    assert!(build_revert_pipeline_steps("not-a-sha").is_err());
}
