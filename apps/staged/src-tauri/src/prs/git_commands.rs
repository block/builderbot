//! The shell commands the pipeline steps run against origin.
//!
//! Each tries SSH first and retries over HTTPS, so a machine without an SSH
//! key for GitHub still fetches and pushes.

const HTTPS_FALLBACK_CONFIG: &str = "url.https://github.com/.insteadOf=git@github.com:";

pub(super) fn git_fetch_with_fallback(refspec: &str) -> String {
    format!(
        "if ! git fetch origin {refspec}; then git -c '{HTTPS_FALLBACK_CONFIG}' fetch origin {refspec}; fi"
    )
}

/// `local:remote` refspec for `git push`. Spelled out even when the names
/// match so every push site reads the same way.
pub(super) fn push_refspec(local_branch: &str, remote_branch: &str) -> String {
    format!("{local_branch}:{remote_branch}")
}

pub(super) fn git_push_with_fallback(args: &str) -> String {
    format!("if ! git push {args}; then git -c '{HTTPS_FALLBACK_CONFIG}' push {args}; fi")
}
