//! The shell commands the pipeline steps run against origin.
//!
//! Each tries the origin as configured first and falls back to HTTPS, so a
//! machine without SSH access to GitHub still fetches and pushes. The two
//! fall back differently:
//!
//! - A fetch retries over HTTPS on any failure. A fetch runs no hook, so a
//!   retry that could not have helped costs one more failed connection.
//! - A push retries only when the origin's push service cannot be opened,
//!   because a push that failed any other way has usually run the
//!   `pre-push` hook already (see [`git_push_with_fallback`]).

const HTTPS_FALLBACK_CONFIG: &str = "url.https://github.com/.insteadOf=git@github.com:";

/// Asks origin for its push service (`git-receive-pack`) and nothing else:
/// no hook runs and nothing is sent. `git push` opens the same service
/// before it runs the `pre-push` hook, so this failing means the push never
/// got as far as the hook.
///
/// It is the push service on purpose. A plain `git ls-remote origin` asks
/// for the fetch service, which GitHub gives to an SSH key that may read a
/// repository but not push to it, and that key needs the HTTPS push too.
///
/// Over HTTPS git ignores `--upload-pack`, warns about it and lists refs
/// through the fetch service. That is harmless: the fallback only rewrites
/// an SSH origin. `HEAD` keeps that listing to one ref. All output is
/// dropped; the failed push has already said what went wrong.
const PUSH_SERVICE_PROBE: &str =
    "git ls-remote --upload-pack=git-receive-pack origin HEAD >/dev/null 2>&1";

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

/// `git push {args}`, retried over HTTPS only if origin's push service
/// cannot be opened.
///
/// A push that fails is not retried blindly: the failure may be the
/// `pre-push` hook's, and a second push runs the hook a second time (a hook
/// that takes 17 minutes to fail then takes 34). So after a failed push,
/// [`PUSH_SERVICE_PROBE`] asks whether the origin's push service opens:
///
/// - It opens: the push got past the connection and failed later, at the
///   hook, as a non-fast-forward, or refused by the server. HTTPS would end
///   the same way, so the step fails with the first push's output.
/// - It does not: the push failed before the hook could run (no SSH key, a
///   key that may not push, port 22 closed), so the HTTPS push is the first
///   to reach the hook.
///
/// The probe runs only after a failure, so a push that works costs no extra
/// connection. The hook can still run twice if origin stops answering
/// between the first push's hook failing and the probe.
pub(super) fn git_push_with_fallback(args: &str) -> String {
    format!(
        "if ! git push {args}; then if {PUSH_SERVICE_PROBE}; then false; else git -c '{HTTPS_FALLBACK_CONFIG}' push {args}; fi; fi"
    )
}
