//! The push step's HTTPS fallback, run for real: the commands the push and
//! create-PR pipelines build, under `sh`, against a local bare origin with a
//! `pre-push` hook that counts its runs.
//!
//! Where a test needs an SSH origin, the clone's origin is
//! `git@github.com:acme/widgets.git` and `GIT_SSH_COMMAND` is a script
//! standing in for `ssh`, so nothing reaches the network. The fallback's
//! `https://github.com/` is swapped for the directory holding the origin
//! (see [`Fixture::localized`]): the fallback push then lands in the same
//! bare repository.

use std::process::{Command, Output};

use super::*;
use crate::test_utils::{run_git_in, TempDir, TempGitRepo};

const SSH_ORIGIN: &str = "git@github.com:acme/widgets.git";
const HOOK_OUTPUT: &str = "pre-push hook ran";

/// What the stand-in for `ssh` does with the service git asks it for.
#[derive(Clone, Copy)]
enum Ssh {
    /// Refuses everything: a machine with no SSH key for GitHub.
    Refused,
    /// Serves fetches and refuses pushes: a key that may read the
    /// repository and not write to it.
    ReadOnly,
    /// Serves both.
    Works,
}

struct Fixture {
    root: TempDir,
    ssh: Option<PathBuf>,
}

impl Fixture {
    /// A bare origin holding `main`, and a clone with one unpushed commit on
    /// a new branch `feature-x`. With `ssh`, the clone's origin is
    /// [`SSH_ORIGIN`]; without, it is the origin's path.
    fn new(ssh: Option<Ssh>) -> Self {
        let seed = TempGitRepo::new();
        seed.write_file("file.txt", "base\n");
        seed.commit("base");

        let fixture = Self {
            root: TempDir::new("push-fallback"),
            ssh: None,
        };
        let origin = fixture.origin();
        std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
        let clone = fixture.clone_path();
        run_git_in(
            seed.path(),
            &["clone", "--bare", ".", origin.to_str().unwrap()],
        );
        run_git_in(
            seed.path(),
            &["clone", origin.to_str().unwrap(), clone.to_str().unwrap()],
        );
        run_git_in(&clone, &["config", "user.email", "test@example.com"]);
        run_git_in(&clone, &["config", "user.name", "Test"]);
        run_git_in(&clone, &["checkout", "-b", "feature-x"]);
        std::fs::write(clone.join("file.txt"), "base\nfeature\n").unwrap();
        run_git_in(&clone, &["commit", "-am", "feature"]);

        // A repo-local hooksPath, so no global one hides the hook.
        let hooks = fixture.root.path().join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        run_git_in(
            &clone,
            &["config", "core.hooksPath", hooks.to_str().unwrap()],
        );
        fixture.set_hook(false);

        let Some(ssh) = ssh else {
            return fixture;
        };
        run_git_in(&clone, &["remote", "set-url", "origin", SSH_ORIGIN]);
        let serve = |service: &str| {
            format!(
                "  \"git-{service} \"*) exec git {service} '{}' ;;\n",
                origin.display()
            )
        };
        let served = match ssh {
            Ssh::Refused => String::new(),
            Ssh::ReadOnly => serve("upload-pack"),
            Ssh::Works => serve("upload-pack") + &serve("receive-pack"),
        };
        // git runs `ssh [options] <host> "<service> '<path>'"`: the service
        // is the last argument. Each request is logged before it is served.
        let script = format!(
            "#!/bin/sh\nfor last; do :; done\necho \"$last\" >> '{log}'\ncase \"$last\" in\n{served}esac\n\
             echo 'ERROR: Permission to acme/widgets.git denied to someone.' >&2\nexit 1\n",
            log = fixture.ssh_log().display(),
        );
        // Named `ssh` so git treats it as OpenSSH and does not probe it.
        let path = fixture.root.path().join("bin").join("ssh");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_executable(&path, &script);
        Self {
            ssh: Some(path),
            ..fixture
        }
    }

    fn origin(&self) -> PathBuf {
        self.root.path().join("acme").join("widgets.git")
    }

    fn clone_path(&self) -> PathBuf {
        self.root.path().join("clone")
    }

    fn ssh_log(&self) -> PathBuf {
        self.root.path().join("ssh.log")
    }

    /// (Re)write the `pre-push` hook: it records the run, says so on stderr,
    /// and passes or fails.
    fn set_hook(&self, passes: bool) {
        let script = format!(
            "#!/bin/sh\necho run >> '{}'\necho '{HOOK_OUTPUT}' >&2\nexit {}\n",
            self.root.path().join("hook-runs").display(),
            if passes { 0 } else { 1 },
        );
        write_executable(&self.root.path().join("hooks").join("pre-push"), &script);
    }

    fn hook_runs(&self) -> usize {
        line_count(&self.root.path().join("hook-runs"))
    }

    /// How many times git asked the stand-in `ssh` for origin's push service.
    fn ssh_push_service_requests(&self) -> usize {
        std::fs::read_to_string(self.ssh_log())
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with("git-receive-pack "))
            .count()
    }

    /// `command` with the fallback's `https://github.com/` replaced by the
    /// directory holding the origin, so `git@github.com:acme/widgets.git`
    /// rewrites to the local bare repository. Panics if the command does not
    /// hold the fallback config: unreplaced, it would push to github.com.
    fn localized(&self, command: &str) -> String {
        let local = format!(
            "url.{}/.insteadOf=git@github.com:",
            self.root.path().display()
        );
        let localized =
            command.replace("url.https://github.com/.insteadOf=git@github.com:", &local);
        assert_eq!(
            localized.matches(&local).count(),
            1,
            "expected one HTTPS fallback in: {command}"
        );
        assert!(!localized.contains("https://"), "{localized}");
        localized
    }

    /// Run a push step's command the way the pipeline does (see
    /// [`Self::sh`]). With an SSH origin, its fallback is made local first.
    fn run(&self, command: &str) -> Output {
        match self.ssh {
            Some(_) => self.sh(&self.localized(command)),
            None => self.sh(command),
        }
    }

    /// `sh -c` in the clone, with the user's and the system's git config out
    /// of the way and the stand-in `ssh`, if the fixture has one.
    fn sh(&self, command: &str) -> Output {
        let mut sh = Command::new("sh");
        sh.arg("-c").arg(command).current_dir(self.clone_path());
        git::strip_git_env(&mut sh);
        sh.env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ssh) = &self.ssh {
            sh.env("GIT_SSH_COMMAND", ssh);
        }
        sh.output().unwrap()
    }

    /// `feature-x` on the origin, or `None` while it has not been pushed.
    fn pushed_tip(&self) -> Option<String> {
        let mut rev_parse = Command::new("git");
        rev_parse.arg("-C").arg(self.origin()).args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/feature-x",
        ]);
        git::strip_git_env(&mut rev_parse);
        let output = rev_parse.output().unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap().trim().to_string())
    }

    fn head(&self) -> String {
        run_git_in(&self.clone_path(), &["rev-parse", "HEAD"])
    }
}

fn write_executable(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn line_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Every push step Staged builds that runs the `pre-push` hook: the push
/// pipeline's normal and force pushes, and the create-PR pipeline's push.
fn verifying_push_commands() -> Vec<(&'static str, String)> {
    let push = |variant| {
        let steps = build_push_pipeline_steps("feature-x", "feature-x", variant);
        command_at(&steps, 0).1.to_string()
    };
    let create_pr =
        build_create_pr_pipeline_steps("pull request", "main", "", "feature-x", "feature-x");
    let (label, create_pr_push, _) = command_at(&create_pr, 4);
    assert_eq!(label, "Push to remote");
    vec![
        ("push", push(PUSH)),
        ("force push", push(FORCE_PUSH)),
        ("create PR push", create_pr_push.to_string()),
    ]
}

/// The failure this fallback used to double: a push whose hook fails. Each
/// push step runs the hook once, shows its output once and pushes nothing.
#[test]
fn a_failing_pre_push_hook_runs_once_for_each_push_step() {
    let fixture = Fixture::new(None);

    for (name, command) in verifying_push_commands() {
        let before = fixture.hook_runs();
        let output = fixture.run(&command);
        let text = output_text(&output);

        assert!(!output.status.success(), "{name}: {text}");
        assert_eq!(fixture.hook_runs(), before + 1, "{name}: {text}");
        assert_eq!(text.matches(HOOK_OUTPUT).count(), 1, "{name}: {text}");
        assert_eq!(fixture.pushed_tip(), None, "{name}");
    }
}

/// The same over an SSH origin that works, which is where the fallback
/// applies: the failed push is followed by one question to origin's push
/// service and no second push.
#[test]
fn a_failing_hook_over_working_ssh_is_not_retried_over_https() {
    let fixture = Fixture::new(Some(Ssh::Works));

    for (name, command) in verifying_push_commands() {
        let (runs, requests) = (fixture.hook_runs(), fixture.ssh_push_service_requests());
        let output = fixture.run(&command);
        let text = output_text(&output);

        assert!(!output.status.success(), "{name}: {text}");
        assert_eq!(fixture.hook_runs(), runs + 1, "{name}: {text}");
        // The push, then the probe.
        assert_eq!(
            fixture.ssh_push_service_requests(),
            requests + 2,
            "{name}: {text}"
        );
        assert_eq!(fixture.pushed_tip(), None, "{name}");
    }

    // With a hook that passes, the first push lands and nothing follows it.
    fixture.set_hook(true);
    let (runs, requests) = (fixture.hook_runs(), fixture.ssh_push_service_requests());
    let output = fixture.run(&verifying_push_commands()[0].1);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(fixture.hook_runs(), runs + 1);
    assert_eq!(fixture.ssh_push_service_requests(), requests + 1);
    assert_eq!(fixture.pushed_tip(), Some(fixture.head()));
}

/// A machine with no SSH access still pushes: the SSH push is refused before
/// the hook, so the fallback's push is the only one to run it.
#[test]
fn a_push_refused_over_ssh_lands_over_the_fallback_and_runs_the_hook_once() {
    let fixture = Fixture::new(Some(Ssh::Refused));

    // A failing hook fails the fallback's push too, after one run.
    let command = &verifying_push_commands()[0].1;
    let output = fixture.run(command);
    assert!(!output.status.success(), "{}", output_text(&output));
    assert_eq!(fixture.hook_runs(), 1, "{}", output_text(&output));
    assert_eq!(fixture.pushed_tip(), None);

    fixture.set_hook(true);
    for (name, command) in verifying_push_commands() {
        let before = fixture.hook_runs();
        let output = fixture.run(&command);
        let text = output_text(&output);

        assert!(output.status.success(), "{name}: {text}");
        assert_eq!(fixture.hook_runs(), before + 1, "{name}: {text}");
        assert_eq!(fixture.pushed_tip(), Some(fixture.head()), "{name}");
    }
}

/// Why the probe asks for the push service and not for a ref listing: a key
/// that may read the repository and not push to it. Listing refs over that
/// SSH works, and the push still has to go over HTTPS.
#[test]
fn a_key_that_can_read_but_not_push_still_falls_back() {
    let fixture = Fixture::new(Some(Ssh::ReadOnly));
    fixture.set_hook(true);

    let listing = fixture.sh("git ls-remote origin HEAD");
    assert!(listing.status.success(), "{}", output_text(&listing));
    assert!(String::from_utf8_lossy(&listing.stdout).contains("HEAD"));

    let output = fixture.run(&verifying_push_commands()[0].1);
    assert!(output.status.success(), "{}", output_text(&output));
    assert_eq!(fixture.hook_runs(), 1);
    assert_eq!(fixture.pushed_tip(), Some(fixture.head()));
}

/// "Push without verifying" never runs the hook, whichever half pushes.
#[test]
fn a_no_verify_push_runs_no_hook_on_either_half_of_the_fallback() {
    for ssh in [Ssh::Works, Ssh::Refused] {
        let fixture = Fixture::new(Some(ssh));
        let steps = build_push_pipeline_steps("feature-x", "feature-x", NO_VERIFY_PUSH);

        let output = fixture.run(command_at(&steps, 0).1);
        assert!(output.status.success(), "{}", output_text(&output));
        assert_eq!(fixture.hook_runs(), 0);
        assert_eq!(fixture.pushed_tip(), Some(fixture.head()));
    }
}
