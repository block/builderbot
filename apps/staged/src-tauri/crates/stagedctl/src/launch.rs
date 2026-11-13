//! Starting Staged for `stagedctl project open` when no instance is running.
//!
//! Every other verb needs an app that is already up. `project open` is the
//! one a user runs to get to Staged, so when nothing answers it starts the
//! app itself, waits for the new instance's control server, and hands back
//! a client for the request to go out on. The app parks that request until
//! its window has loaded (`src-tauri/src/project_open.rs`).
//!
//! What may be started, and for whom:
//!
//! - **Only the bundle this executable is in.** `stagedctl` ships as a
//!   sidecar at `Staged.app/Contents/MacOS/stagedctl`, so the bundle is read
//!   off the executable's own resolved path ([`bundle_for_executable`]).
//!   Nothing from the arguments or the environment names what is started,
//!   and `open` is run by absolute path.
//! - **Never a development build.** Its executable is not inside a bundle,
//!   and a dev binary needs its Vite server; the user is told to run
//!   `just dev` (exit 3).
//! - **Never for a caller with no credential.** A grant can only be proven
//!   to a running app, so the check before launching is that an unexpired
//!   grant file exists on this machine. Without one the app would start and
//!   then refuse the command; instead nothing starts and the caller is told
//!   to run `stagedctl connect` (exit 4). The consent dialog is not raised
//!   on the caller's behalf.
//! - **Never when a control file was named.** `--lock-path` and
//!   `$STAGEDCTL_LOCK` point at one instance; a fresh app cannot make that
//!   file valid. So a spawned session never starts an app.
//! - **Never over an app that is there but unusable.** A running Staged
//!   that speaks another protocol version, say, is reported as it always
//!   was. Only [`CtlError::NotRunning`] from every candidate counts as "not
//!   running".
//!
//! The pieces are kept apart so the tests never start the real app: the
//! decision ([`launch_target`]), the bundle path ([`bundle_for_executable`])
//! and the wait ([`wait_for_instance`]) take what they need as arguments,
//! and the one function that spawns a process sits behind [`Launcher`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::client::{
    discover_and_connect, live_discovery_candidates, Client, CtlError, DiscoveryCandidate,
};
use crate::grants::{self, GrantFile};

/// How long a started Staged is given to bring its control server up. The
/// server starts during app setup, ahead of the window's first paint, so
/// this covers a cold start with room to spare.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// The pause between looks for the new instance's discovery file.
const LAUNCH_POLL_PAUSE: Duration = Duration::from_millis(200);

/// The step that starts a process. Everything else in this module is
/// reachable without one.
pub trait Launcher {
    /// Start the app bundle at `bundle` and return once the request to
    /// start it has been made, not once the app is up. An `Err` is the
    /// reason, for the user.
    fn launch(&self, bundle: &Path) -> Result<(), String>;
}

/// The real [`Launcher`]: macOS `open`, by absolute path so `$PATH` cannot
/// substitute another program. LaunchServices starts the app, so it does
/// not inherit this process's environment.
pub struct OpenCommand;

/// Where macOS keeps `open`.
#[cfg(any(target_os = "macos", test))]
const OPEN_PROGRAM: &str = "/usr/bin/open";

/// The `open` invocation for `bundle`: the bundle path and nothing else.
#[cfg(any(target_os = "macos", test))]
fn open_command(bundle: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(OPEN_PROGRAM);
    command
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    command
}

impl Launcher for OpenCommand {
    #[cfg(target_os = "macos")]
    fn launch(&self, bundle: &Path) -> Result<(), String> {
        let output = open_command(bundle)
            .output()
            .map_err(|e| format!("cannot run {OPEN_PROGRAM}: {e}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        Err(if stderr.is_empty() {
            format!("{OPEN_PROGRAM} failed ({})", output.status)
        } else {
            format!("{OPEN_PROGRAM} failed ({}): {stderr}", output.status)
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn launch(&self, _bundle: &Path) -> Result<(), String> {
        Err("starting Staged from the command line is only supported on macOS".to_string())
    }
}

/// What the launch path takes from the machine it runs on. Tests supply
/// their own, so no real app is started and no wait runs to its full
/// length.
pub struct LaunchEnv<'a> {
    /// This executable's path with symlinks resolved (`/usr/local/bin/
    /// stagedctl` is a link into the bundle), or `None` when the OS could
    /// not say.
    pub executable: Option<PathBuf>,
    pub launcher: &'a dyn Launcher,
    /// How long to wait for the started app's control server.
    pub timeout: Duration,
    /// The pause between looks for it.
    pub poll_pause: Duration,
}

impl LaunchEnv<'static> {
    /// The running process's own: its executable, `open`, and the default
    /// wait.
    pub fn real() -> Self {
        Self {
            executable: std::env::current_exe()
                .and_then(|path| path.canonicalize())
                .ok(),
            launcher: &OpenCommand,
            timeout: LAUNCH_TIMEOUT,
            poll_pause: LAUNCH_POLL_PAUSE,
        }
    }
}

/// The app bundle `executable` is the sidecar of:
/// `<bundle>.app/Contents/MacOS/<executable>` gives `<bundle>.app`.
///
/// Purely lexical; the caller resolves symlinks first. Anything else names
/// no bundle: a development build under `target/`, a loose copy of the
/// binary, a relative path, or a path that climbs with `..`.
pub(crate) fn bundle_for_executable(executable: &Path) -> Option<&Path> {
    if !executable.is_absolute() {
        return None;
    }
    executable.file_name()?;
    let macos = executable.parent()?;
    if macos.file_name()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bundle = contents.parent()?;
    (bundle.extension()? == "app").then_some(bundle)
}

/// Whether a grant on this machine could still authenticate. This is all
/// that can be checked with no app to prove a grant to: the file exists and
/// has not expired. Whether the app still honours it, and whether it
/// reaches the project, is the app's to say once it is up.
pub(crate) fn has_unexpired_grant(grants: &[GrantFile], now_ms: i64) -> bool {
    grants.iter().any(|grant| !grant.is_expired_at(now_ms))
}

/// Decide what to do about a Staged that is not running: the bundle to
/// start, or the reason nothing is started.
///
/// In order: `--no-launch` (exit 3), then no credential (exit 4,
/// `authorization_required`), then no bundle to start (exit 3). `grants` is
/// only read when `--no-launch` did not already settle it.
pub(crate) fn launch_target(
    no_launch: bool,
    grants: impl FnOnce() -> Vec<GrantFile>,
    now_ms: i64,
    executable: Option<&Path>,
) -> Result<PathBuf, CtlError> {
    if no_launch {
        return Err(CtlError::NotRunning(
            "Staged is not running, and --no-launch says not to start it; start Staged and run \
             the command again"
                .to_string(),
        ));
    }
    if !has_unexpired_grant(&grants(), now_ms) {
        return Err(CtlError::Authorization {
            code: "authorization_required".to_string(),
            message: "Staged is not running, and this machine holds no unexpired connected-app \
                      grant to open a project with, so Staged was not started; start Staged, \
                      run `stagedctl connect --client <name>` once, then retry"
                .to_string(),
        });
    }
    let Some(executable) = executable else {
        return Err(CtlError::Environment(
            "Staged is not running, and stagedctl could not determine its own location to \
             find the app it belongs to; start Staged and run the command again"
                .to_string(),
        ));
    };
    match bundle_for_executable(executable) {
        Some(bundle) => Ok(bundle.to_path_buf()),
        None => Err(CtlError::Environment(format!(
            "Staged is not running, and this stagedctl ({}) is not inside a Staged.app bundle, \
             so it has no app to start; a development build needs its dev server, so run \
             `just dev`, or start Staged, and run the command again",
            executable.display()
        ))),
    }
}

/// Connect to the first of `candidates` that passes the handshake.
///
/// `Ok(None)` means no Staged is running: there are no candidates, or every
/// one failed as [`CtlError::NotRunning`] (a crashed instance's leftover
/// directory, say). A candidate that failed any other way is an app that is
/// there but unusable; the newest such failure is the error, and nothing is
/// started over it.
fn find_running(
    candidates: &[DiscoveryCandidate],
    mut connect: impl FnMut(&Path) -> Result<Client, CtlError>,
) -> Result<Option<Client>, CtlError> {
    let mut unusable: Option<CtlError> = None;
    for candidate in candidates {
        match connect(&candidate.path) {
            Ok(client) => return Ok(Some(client)),
            Err(CtlError::NotRunning(_)) => {}
            Err(e) => {
                unusable.get_or_insert(e);
            }
        }
    }
    match unusable {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

/// Wait for a started Staged: poll `candidates` until one that is not in
/// `stale` passes the handshake, or `timeout` runs out (exit 3).
///
/// `stale` is what was on disk before the launch, every entry already found
/// dead. Those are skipped, so a leftover directory costs nothing per poll;
/// a file the app has rewritten since (same path, new mtime) is new again.
/// A new candidate that is not answering yet is tried again next round. One
/// that fails any other way will not get better by waiting, so its error is
/// returned at once.
fn wait_for_instance(
    stale: &[DiscoveryCandidate],
    timeout: Duration,
    poll_pause: Duration,
    mut candidates: impl FnMut() -> Vec<DiscoveryCandidate>,
    mut connect: impl FnMut(&Path) -> Result<Client, CtlError>,
) -> Result<Client, CtlError> {
    let deadline = Instant::now() + timeout;
    loop {
        for candidate in candidates() {
            if stale.contains(&candidate) {
                continue;
            }
            match connect(&candidate.path) {
                Ok(client) => return Ok(client),
                Err(CtlError::NotRunning(_)) => {}
                Err(e) => return Err(e),
            }
        }
        if Instant::now() >= deadline {
            return Err(launch_timed_out(timeout));
        }
        std::thread::sleep(poll_pause);
    }
}

/// The error for a started Staged whose control server never answered. Not
/// [`CtlError::NotRunning`]: the app was started, so starting it again is
/// not the remedy.
fn launch_timed_out(timeout: Duration) -> CtlError {
    CtlError::Environment(format!(
        "Staged was started but its control server did not answer within {} seconds; run the \
         command again once its window is up",
        timeout.as_secs()
    ))
}

/// A verified, authenticated client for `project open`, starting Staged
/// first when none is running. `lock_flag` and `lock_env` are `--lock-path`
/// and `$STAGEDCTL_LOCK`; `data_dir` is `~/.staged`.
///
/// With an explicit control file this is [`discover_and_connect`] and
/// nothing is ever started. Otherwise each live instance is tried, newest
/// first, as for every other verb, and only when none is running does
/// [`launch_target`] decide whether to start one.
pub fn connect_or_launch(
    lock_flag: Option<&Path>,
    lock_env: Option<OsString>,
    data_dir: Option<&Path>,
    no_launch: bool,
    env: &LaunchEnv<'_>,
) -> Result<Client, CtlError> {
    let data_dir = match data_dir {
        Some(data_dir) if lock_flag.is_none() && lock_env.is_none() => data_dir,
        _ => return discover_and_connect(lock_flag, lock_env, data_dir),
    };
    let before = live_discovery_candidates(data_dir);
    let mut client = match find_running(&before, Client::connect)? {
        Some(client) => client,
        None => {
            let bundle = launch_target(
                no_launch,
                || {
                    grants::read_grant_files(data_dir)
                        .into_iter()
                        .map(|(_, grant)| grant)
                        .collect()
                },
                grants::now_ms(),
                env.executable.as_deref(),
            )?;
            eprintln!(
                "stagedctl: Staged is not running; starting {}",
                bundle.display()
            );
            env.launcher.launch(&bundle).map_err(|reason| {
                CtlError::Environment(format!(
                    "could not start Staged at {}: {reason}; start it and run the command again",
                    bundle.display()
                ))
            })?;
            wait_for_instance(
                &before,
                env.timeout,
                env.poll_pause,
                || live_discovery_candidates(data_dir),
                Client::connect,
            )?
        }
    };
    // Every candidate is an instance's `discovery.json`, which carries no
    // session credential, so the grant lookup applies.
    client.attach_grant_credential(data_dir)?;
    Ok(client)
}

// `pub(crate)` so the `project open` tests can reuse the fake launchers.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::client::tests::{
        fake_server, grant_aware_server, grant_file, ping_body, private_tempdir, set_mtime,
        write_tcp_discovery,
    };
    use crate::client::PROTOCOL_VERSION;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    // -------------------------------------------------------------------
    // Fixtures. Nothing here starts a process: the launchers are fakes, and
    // an "app" is a fake control server plus the discovery file a real one
    // would write.
    // -------------------------------------------------------------------

    /// Where an installed `stagedctl` resolves to. Only ever read as text.
    pub(crate) const BUNDLED_EXECUTABLE: &str = "/Applications/Staged.app/Contents/MacOS/stagedctl";
    pub(crate) const BUNDLE: &str = "/Applications/Staged.app";
    /// Where a development build's `stagedctl` is.
    const DEV_EXECUTABLE: &str =
        "/Users/me/builderbot/apps/staged/src-tauri/target/debug/stagedctl";

    /// A launcher that must not be reached.
    pub(crate) struct NeverLaunch;

    impl Launcher for NeverLaunch {
        fn launch(&self, bundle: &Path) -> Result<(), String> {
            panic!(
                "Staged was started ({}), which must not happen",
                bundle.display()
            )
        }
    }

    /// A launcher that records what it was asked to start and then runs
    /// `on_launch` in place of the app coming up.
    pub(crate) struct FakeLauncher<F> {
        pub(crate) launched: Mutex<Vec<PathBuf>>,
        on_launch: F,
    }

    impl<F: Fn() -> Result<(), String>> FakeLauncher<F> {
        pub(crate) fn new(on_launch: F) -> Self {
            Self {
                launched: Mutex::new(Vec::new()),
                on_launch,
            }
        }

        pub(crate) fn launched(&self) -> Vec<PathBuf> {
            self.launched.lock().unwrap().clone()
        }
    }

    impl<F: Fn() -> Result<(), String>> Launcher for FakeLauncher<F> {
        fn launch(&self, bundle: &Path) -> Result<(), String> {
            self.launched.lock().unwrap().push(bundle.to_path_buf());
            (self.on_launch)()
        }
    }

    /// A launch environment for tests: a wait long enough for a loaded
    /// machine, polled often.
    pub(crate) fn test_env<'a>(
        executable: Option<&str>,
        launcher: &'a dyn Launcher,
    ) -> LaunchEnv<'a> {
        LaunchEnv {
            executable: executable.map(PathBuf::from),
            launcher,
            timeout: Duration::from_secs(20),
            poll_pause: Duration::from_millis(20),
        }
    }

    /// An unexpired grant id the fake app knows, and its secret.
    pub(crate) const GRANT_ID: &str = "4e77e4";
    const GRANT_SECRET: &str = "s3cret";

    /// Store an unexpired grant under `data_dir`, as `stagedctl connect`
    /// would have.
    pub(crate) fn store_grant(data_dir: &Path) {
        let expires_at = grants::now_ms() + 60 * 60 * 1000;
        grants::write_grant_file(data_dir, &grant_file(GRANT_ID, GRANT_SECRET, expires_at))
            .unwrap();
    }

    /// A fake Staged that knows [`GRANT_ID`], listening but not yet
    /// discoverable: its port, for the discovery file to name.
    fn grant_aware_app() -> u16 {
        let renew_to = grants::now_ms() + grants::GRANT_LIFETIME_MS - 1000;
        let (port, _handle) = grant_aware_server(
            std::process::id(),
            vec![(GRANT_ID.to_string(), GRANT_SECRET.to_string())],
            renew_to,
            Arc::new(Mutex::new(Vec::new())),
        );
        port
    }

    /// A port nothing listens on.
    fn closed_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// A crashed instance's leftover: pid 1 is alive, so the directory
    /// looks live, but nothing answers at its port.
    fn write_zombie(data_dir: &Path) -> PathBuf {
        write_tcp_discovery(data_dir, 1, closed_port(), 1)
    }

    fn candidate(path: &str) -> DiscoveryCandidate {
        DiscoveryCandidate {
            path: PathBuf::from(path),
            modified: std::time::UNIX_EPOCH,
        }
    }

    fn not_running() -> CtlError {
        CtlError::NotRunning("nothing there".to_string())
    }

    // -------------------------------------------------------------------
    // Bundle path
    // -------------------------------------------------------------------

    fn bundle_of(executable: &str) -> Option<&str> {
        bundle_for_executable(Path::new(executable)).and_then(Path::to_str)
    }

    #[test]
    fn a_sidecar_names_the_bundle_it_is_in() {
        for (executable, bundle) in [
            (BUNDLED_EXECUTABLE, BUNDLE),
            (
                "/Users/me/Applications/Staged.app/Contents/MacOS/stagedctl",
                "/Users/me/Applications/Staged.app",
            ),
            // A bundle built in the repo.
            (
                "/Users/me/builderbot/apps/staged/src-tauri/target/release/bundle/macos/Staged.app/Contents/MacOS/stagedctl",
                "/Users/me/builderbot/apps/staged/src-tauri/target/release/bundle/macos/Staged.app",
            ),
            // Renamed by the user, with a space.
            (
                "/Applications/Staged Beta.app/Contents/MacOS/stagedctl",
                "/Applications/Staged Beta.app",
            ),
            // The bundle is found by where the executable is, not by what
            // the executable is called.
            (
                "/Applications/Staged.app/Contents/MacOS/stagedctl-aarch64-apple-darwin",
                BUNDLE,
            ),
            // A bundle inside another bundle is the inner one.
            (
                "/Applications/Outer.app/Contents/Resources/Staged.app/Contents/MacOS/stagedctl",
                "/Applications/Outer.app/Contents/Resources/Staged.app",
            ),
        ] {
            assert_eq!(bundle_of(executable), Some(bundle), "{executable}");
        }
    }

    #[test]
    fn an_executable_outside_a_bundle_names_none() {
        for executable in [
            // Development builds.
            DEV_EXECUTABLE,
            "/Users/me/.cache/cargo-target/release/stagedctl",
            // An install link that was not resolved, or a loose copy.
            "/usr/local/bin/stagedctl",
            "/Users/me/.staged/control/4242/bin/stagedctl",
            // The right segments in the wrong place.
            "/Applications/Staged.app/Contents/Resources/stagedctl",
            "/Applications/Staged.app/Contents/MacOS/helpers/stagedctl",
            "/Applications/Staged.app/MacOS/stagedctl",
            "/Applications/Staged.app/stagedctl",
            "/Applications/Staged/Contents/MacOS/stagedctl",
            // Segment names are matched exactly.
            "/Applications/Staged.app/Contents/macos/stagedctl",
            "/Applications/Staged.app/contents/MacOS/stagedctl",
            "/Applications/Staged.APP/Contents/MacOS/stagedctl",
            "/Applications/Staged.application/Contents/MacOS/stagedctl",
            // A directory called `.app` has no extension.
            "/Applications/.app/Contents/MacOS/stagedctl",
            // The directory itself is not an executable in it.
            "/Applications/Staged.app/Contents/MacOS",
            "/Applications/Staged.app/Contents/MacOS/",
            // Where a path that climbs lands cannot be told from its text.
            "/Applications/Staged.app/Contents/MacOS/../stagedctl",
            "/Applications/Staged.app/Contents/MacOS/..",
            // A relative path is never handed to `open`.
            "Staged.app/Contents/MacOS/stagedctl",
            "./Staged.app/Contents/MacOS/stagedctl",
            "stagedctl",
            "",
            "/",
        ] {
            assert_eq!(bundle_of(executable), None, "{executable}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_install_link_resolves_to_the_bundle_it_points_into() {
        // `/usr/local/bin/stagedctl` is a symlink to the sidecar. The OS
        // may report the link as the executable, so `LaunchEnv::real`
        // resolves it before the bundle is read off the path.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let macos = root.join("Applications/Staged.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("stagedctl"), "").unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let link = root.join("bin/stagedctl");
        std::os::unix::fs::symlink(macos.join("stagedctl"), &link).unwrap();

        assert_eq!(bundle_for_executable(&link), None, "the link as written");
        let resolved = link.canonicalize().unwrap();
        assert_eq!(
            bundle_for_executable(&resolved),
            Some(root.join("Applications/Staged.app").as_path())
        );
    }

    // -------------------------------------------------------------------
    // The spawn, without spawning
    // -------------------------------------------------------------------

    #[test]
    fn open_is_run_by_absolute_path_with_the_bundle_and_nothing_else() {
        let bundle = Path::new("/Applications/Staged Beta.app");
        let command = open_command(bundle);
        assert_eq!(command.get_program(), "/usr/bin/open");
        // One argument, spaces and all: no shell, no flags, no project.
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, vec![bundle.as_os_str()]);
        assert_eq!(command.get_envs().count(), 0, "no environment is added");
    }

    // -------------------------------------------------------------------
    // The decision: start Staged, or say why not
    // -------------------------------------------------------------------

    const NOW: i64 = 1_800_000_000_000;

    fn unexpired() -> Vec<GrantFile> {
        vec![grant_file(GRANT_ID, GRANT_SECRET, NOW + 1)]
    }

    fn no_grants_read() -> Vec<GrantFile> {
        panic!("the grants were read, though --no-launch had already settled it")
    }

    #[test]
    fn a_grant_and_a_bundle_start_that_bundle() {
        let bundle =
            launch_target(false, unexpired, NOW, Some(Path::new(BUNDLED_EXECUTABLE))).unwrap();
        assert_eq!(bundle, Path::new(BUNDLE));
    }

    #[test]
    fn no_launch_is_exit_3_whatever_else_is_true() {
        for executable in [Some(BUNDLED_EXECUTABLE), Some(DEV_EXECUTABLE), None] {
            let err =
                launch_target(true, no_grants_read, NOW, executable.map(Path::new)).unwrap_err();
            assert_eq!(err.exit_code(), 3, "{err}");
            assert!(matches!(err, CtlError::NotRunning(_)), "{err:?}");
            assert_eq!(
                err.to_string(),
                "Staged is not running, and --no-launch says not to start it; start Staged and \
                 run the command again"
            );
        }
    }

    #[test]
    fn the_credential_precheck_wants_one_unexpired_grant() {
        assert!(!has_unexpired_grant(&[], NOW));
        // A grant expires at its `expires_at`, not after it.
        assert!(!has_unexpired_grant(&[grant_file("a1", "s", NOW)], NOW));
        assert!(!has_unexpired_grant(&[grant_file("a1", "s", NOW - 1)], NOW));
        assert!(has_unexpired_grant(&[grant_file("a1", "s", NOW + 1)], NOW));
        // One live grant among dead ones is enough.
        assert!(has_unexpired_grant(
            &[
                grant_file("a1", "s", NOW - 1),
                grant_file("b2", "s", NOW + 1),
                grant_file("c3", "s", 0),
            ],
            NOW
        ));
    }

    #[test]
    fn no_credential_is_exit_4_and_points_at_connect() {
        let expired = grant_file(GRANT_ID, GRANT_SECRET, NOW - 1);
        for (case, grants) in [
            ("no grant files", Vec::new()),
            ("only an expired grant", vec![expired]),
        ] {
            let grants = move || grants;
            // Even from a bundle: the caller could not use the app it
            // would start.
            let err =
                launch_target(false, grants, NOW, Some(Path::new(BUNDLED_EXECUTABLE))).unwrap_err();
            assert_eq!(err.exit_code(), 4, "{case}: {err}");
            assert_eq!(err.code(), "authorization_required", "{case}");
            assert_eq!(
                err.to_string(),
                "authorization_required: Staged is not running, and this machine holds no \
                 unexpired connected-app grant to open a project with, so Staged was not \
                 started; start Staged, run `stagedctl connect --client <name>` once, then retry",
                "{case}"
            );
        }
    }

    #[test]
    fn the_credential_is_checked_before_the_bundle() {
        // No grant and no bundle: the answer is the credential (exit 4),
        // so an unauthorized caller learns nothing about the install.
        for executable in [Some(DEV_EXECUTABLE), None] {
            let err = launch_target(false, Vec::new, NOW, executable.map(Path::new)).unwrap_err();
            assert_eq!(err.exit_code(), 4, "{err}");
        }
    }

    #[test]
    fn a_development_build_is_exit_3_with_the_just_dev_hint() {
        let err =
            launch_target(false, unexpired, NOW, Some(Path::new(DEV_EXECUTABLE))).unwrap_err();
        assert_eq!(err.exit_code(), 3, "{err}");
        let message = err.to_string();
        assert!(message.contains("`just dev`"), "got: {message}");
        assert!(message.contains(DEV_EXECUTABLE), "got: {message}");
        assert!(
            message.contains("not inside a Staged.app bundle"),
            "got: {message}"
        );
    }

    #[test]
    fn an_executable_the_os_could_not_name_is_exit_3() {
        let err = launch_target(false, unexpired, NOW, None).unwrap_err();
        assert_eq!(err.exit_code(), 3, "{err}");
        assert!(
            err.to_string()
                .contains("could not determine its own location"),
            "got: {err}"
        );
    }

    // -------------------------------------------------------------------
    // Is a Staged running?
    // -------------------------------------------------------------------

    #[test]
    fn no_candidates_or_only_dead_ones_mean_not_running() {
        let none = find_running(&[], |path| panic!("probed {}", path.display())).unwrap();
        assert!(none.is_none());

        let probed = AtomicUsize::new(0);
        let dead = find_running(&[candidate("/a"), candidate("/b")], |_| {
            probed.fetch_add(1, Ordering::SeqCst);
            Err(not_running())
        })
        .unwrap();
        assert!(dead.is_none());
        assert_eq!(probed.load(Ordering::SeqCst), 2, "every candidate is tried");
    }

    #[test]
    fn an_app_that_is_there_but_unusable_is_an_error_not_an_absence() {
        // The newest candidate is a leftover; an older one answers but
        // cannot be used. That is not "not running", so nothing may be
        // started over it, and its error is the one reported.
        let err = find_running(
            &[candidate("/zombie"), candidate("/updated"), candidate("/c")],
            |path| {
                Err(match path.to_str().unwrap() {
                    "/zombie" => not_running(),
                    "/updated" => CtlError::Environment("app updated".to_string()),
                    _ => CtlError::Environment("a later failure".to_string()),
                })
            },
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "app updated");
        assert_eq!(err.exit_code(), 3);
    }

    #[test]
    fn the_first_instance_that_answers_is_the_one_and_later_ones_are_not_tried() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 1, PROTOCOL_VERSION)));
        let dir = private_tempdir();
        let live = write_tcp_discovery(dir.path(), pid, port, 1);
        let candidates = [
            DiscoveryCandidate {
                path: live.clone(),
                modified: std::time::UNIX_EPOCH,
            },
            candidate("/never"),
        ];

        let client = find_running(&candidates, |path| {
            assert_eq!(path, live, "only the first candidate is probed");
            Client::connect(path)
        })
        .unwrap()
        .expect("the fake app answers");
        assert_eq!(client.pid, pid);
    }

    // -------------------------------------------------------------------
    // The wait
    // -------------------------------------------------------------------

    #[test]
    fn the_wait_finds_an_app_whose_discovery_file_appears_later() {
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 7, PROTOCOL_VERSION)));
        let dir = private_tempdir();
        let data_dir = dir.path().to_path_buf();
        let polls = AtomicUsize::new(0);

        // The app "comes up" after a few empty polls.
        let writer = {
            let data_dir = data_dir.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                write_tcp_discovery(&data_dir, pid, port, 7);
            })
        };
        let client = wait_for_instance(
            &[],
            Duration::from_secs(20),
            Duration::from_millis(20),
            || {
                polls.fetch_add(1, Ordering::SeqCst);
                live_discovery_candidates(&data_dir)
            },
            Client::connect,
        )
        .unwrap();
        writer.join().unwrap();

        assert_eq!((client.pid, client.generation), (pid, 7));
        assert!(
            polls.load(Ordering::SeqCst) > 1,
            "the wait polled before the file was there"
        );
    }

    #[test]
    fn the_wait_never_probes_what_was_already_there_and_times_out_as_exit_3() {
        let dir = private_tempdir();
        let zombie = write_zombie(dir.path());
        let stale = live_discovery_candidates(dir.path());
        assert_eq!(stale.len(), 1);

        let started = Instant::now();
        let err = wait_for_instance(
            &stale,
            Duration::from_millis(200),
            Duration::from_millis(20),
            || live_discovery_candidates(dir.path()),
            |path| -> Result<Client, CtlError> {
                panic!("probed the stale {} (zombie is {zombie:?})", path.display())
            },
        )
        .unwrap_err();

        assert!(started.elapsed() >= Duration::from_millis(200));
        assert_eq!(err.exit_code(), 3);
        assert!(
            matches!(err, CtlError::Environment(_)),
            "a started app that never answered is not `NotRunning`: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            launch_timed_out(Duration::from_millis(200)).to_string()
        );
    }

    #[test]
    fn the_timeout_message_names_the_wait_and_the_remedy() {
        assert_eq!(
            launch_timed_out(LAUNCH_TIMEOUT).to_string(),
            "Staged was started but its control server did not answer within 30 seconds; run \
             the command again once its window is up"
        );
    }

    #[test]
    fn the_wait_treats_a_rewritten_file_at_a_stale_path_as_new() {
        // The started app was handed a pid a leftover directory already
        // had, and rewrote the file there: same path, new mtime.
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 2, PROTOCOL_VERSION)));
        let dir = private_tempdir();
        let path = write_tcp_discovery(dir.path(), pid, closed_port(), 1);
        set_mtime(&path, Duration::from_secs(600));
        let stale = live_discovery_candidates(dir.path());

        write_tcp_discovery(dir.path(), pid, port, 2);
        let client = wait_for_instance(
            &stale,
            Duration::from_secs(20),
            Duration::from_millis(20),
            || live_discovery_candidates(dir.path()),
            Client::connect,
        )
        .unwrap();
        assert_eq!(client.generation, 2);
    }

    #[test]
    fn the_wait_retries_an_app_that_is_not_answering_yet() {
        // The discovery file is on disk before the server accepts.
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 1, PROTOCOL_VERSION)));
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), pid, port, 1);
        let attempts = AtomicUsize::new(0);

        let client = wait_for_instance(
            &[],
            Duration::from_secs(20),
            Duration::from_millis(5),
            || live_discovery_candidates(dir.path()),
            |path| {
                if attempts.fetch_add(1, Ordering::SeqCst) < 3 {
                    return Err(not_running());
                }
                Client::connect(path)
            },
        )
        .unwrap();
        assert_eq!(client.pid, pid);
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn the_wait_gives_up_at_once_on_an_app_that_answers_but_cannot_be_used() {
        let dir = private_tempdir();
        write_tcp_discovery(dir.path(), std::process::id(), 1, 1);
        let attempts = AtomicUsize::new(0);

        let err = wait_for_instance(
            &[],
            // Long enough that a retry loop would hang the test.
            Duration::from_secs(600),
            Duration::from_millis(5),
            || live_discovery_candidates(dir.path()),
            |_| -> Result<Client, CtlError> {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(CtlError::Environment("app updated".to_string()))
            },
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "app updated");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    // -------------------------------------------------------------------
    // Connect, starting Staged when none is running
    // -------------------------------------------------------------------

    #[test]
    fn a_running_staged_is_used_and_nothing_is_started() {
        let dir = private_tempdir();
        store_grant(dir.path());
        write_tcp_discovery(dir.path(), std::process::id(), grant_aware_app(), 1);

        for no_launch in [false, true] {
            let client = connect_or_launch(
                None,
                None,
                Some(dir.path()),
                no_launch,
                &test_env(Some(BUNDLED_EXECUTABLE), &NeverLaunch),
            )
            .unwrap();
            assert_eq!(client.pid, std::process::id());
            assert_eq!(
                client.bearer(),
                Some(format!("live-{GRANT_ID}").as_str()),
                "the grant is attached as for every other verb"
            );
        }
    }

    #[test]
    fn with_none_running_the_bundle_is_started_and_the_new_instance_is_used() {
        let dir = private_tempdir();
        store_grant(dir.path());
        let port = grant_aware_app();
        let data_dir = dir.path().to_path_buf();
        // The app coming up: its discovery file lands a little after the
        // launch returns, as a real start's does.
        let launcher = FakeLauncher::new(move || {
            let data_dir = data_dir.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                write_tcp_discovery(&data_dir, std::process::id(), port, 1);
            });
            Ok(())
        });

        let client = connect_or_launch(
            None,
            None,
            Some(dir.path()),
            false,
            &test_env(Some(BUNDLED_EXECUTABLE), &launcher),
        )
        .unwrap();

        assert_eq!(
            launcher.launched(),
            vec![PathBuf::from(BUNDLE)],
            "started once, and only the CLI's own bundle"
        );
        assert_eq!(client.pid, std::process::id());
        assert_eq!(client.bearer(), Some(format!("live-{GRANT_ID}").as_str()));
    }

    #[test]
    fn a_leftover_instance_directory_does_not_count_as_running() {
        // Only a zombie on disk: Staged is started, and the zombie is
        // neither mistaken for the new instance nor in its way.
        let dir = private_tempdir();
        store_grant(dir.path());
        write_zombie(dir.path());
        let port = grant_aware_app();
        let data_dir = dir.path().to_path_buf();
        let launcher = FakeLauncher::new(move || {
            write_tcp_discovery(&data_dir, std::process::id(), port, 1);
            Ok(())
        });

        let client = connect_or_launch(
            None,
            None,
            Some(dir.path()),
            false,
            &test_env(Some(BUNDLED_EXECUTABLE), &launcher),
        )
        .unwrap();
        assert_eq!(launcher.launched(), vec![PathBuf::from(BUNDLE)]);
        assert_eq!(client.pid, std::process::id());
    }

    /// `connect_or_launch` with nothing running and a launcher that must
    /// not be reached.
    fn refused(data_dir: &Path, no_launch: bool, executable: Option<&str>) -> CtlError {
        connect_or_launch(
            None,
            None,
            Some(data_dir),
            no_launch,
            &test_env(executable, &NeverLaunch),
        )
        .unwrap_err()
    }

    #[test]
    fn no_launch_exits_3_without_starting_anything() {
        let dir = private_tempdir();
        store_grant(dir.path());
        let err = refused(dir.path(), true, Some(BUNDLED_EXECUTABLE));
        assert_eq!(err.exit_code(), 3, "{err}");
        assert!(err.to_string().contains("--no-launch"), "got: {err}");

        // A leftover directory changes nothing.
        write_zombie(dir.path());
        let err = refused(dir.path(), true, Some(BUNDLED_EXECUTABLE));
        assert_eq!(err.exit_code(), 3, "{err}");
        assert!(err.to_string().contains("--no-launch"), "got: {err}");
    }

    #[test]
    fn a_caller_with_no_grant_never_starts_staged() {
        let dir = private_tempdir();
        let err = refused(dir.path(), false, Some(BUNDLED_EXECUTABLE));
        assert_eq!(err.exit_code(), 4, "{err}");
        assert_eq!(err.code(), "authorization_required");
        assert!(
            err.to_string().contains("stagedctl connect --client"),
            "got: {err}"
        );

        // Nor with only a grant that has run out.
        let expired = grant_file(GRANT_ID, GRANT_SECRET, grants::now_ms() - 1);
        grants::write_grant_file(dir.path(), &expired).unwrap();
        let err = refused(dir.path(), false, Some(BUNDLED_EXECUTABLE));
        assert_eq!(err.code(), "authorization_required", "{err}");
    }

    #[test]
    fn a_development_build_never_starts_staged() {
        let dir = private_tempdir();
        store_grant(dir.path());
        let err = refused(dir.path(), false, Some(DEV_EXECUTABLE));
        assert_eq!(err.exit_code(), 3, "{err}");
        assert!(err.to_string().contains("`just dev`"), "got: {err}");
    }

    #[test]
    fn a_staged_that_is_running_but_unusable_is_reported_and_none_is_started() {
        // An app from another version answers the ping. Starting a second
        // Staged beside it is not the remedy; the usual message is.
        let pid = std::process::id();
        let (port, _handle) = fake_server(move |_| (200, ping_body(pid, 1, PROTOCOL_VERSION + 1)));
        let dir = private_tempdir();
        store_grant(dir.path());
        write_tcp_discovery(dir.path(), pid, port, 1);

        let err = refused(dir.path(), false, Some(BUNDLED_EXECUTABLE));
        assert_eq!(err.exit_code(), 3);
        assert!(
            err.to_string().contains("app updated — restart Staged"),
            "got: {err}"
        );
    }

    #[test]
    fn a_named_control_file_never_starts_staged() {
        // `--lock-path` and `$STAGEDCTL_LOCK` point at one instance. When
        // it is gone the error is the one every verb gives, even though a
        // grant and a bundle are both to hand.
        let dir = private_tempdir();
        store_grant(dir.path());
        let gone = dir.path().join("control/4242/sessions/s1.json");
        let env = test_env(Some(BUNDLED_EXECUTABLE), &NeverLaunch);

        let by_flag =
            connect_or_launch(Some(&gone), None, Some(dir.path()), false, &env).unwrap_err();
        assert_eq!(by_flag.exit_code(), 3, "{by_flag}");
        assert!(
            by_flag.to_string().contains("cannot read control file"),
            "got: {by_flag}"
        );

        let by_env = connect_or_launch(
            None,
            Some(gone.clone().into_os_string()),
            Some(dir.path()),
            false,
            &env,
        )
        .unwrap_err();
        assert_eq!(by_env.to_string(), by_flag.to_string());

        // An empty `$STAGEDCTL_LOCK` is still an error, not a fallback
        // that could reach the launch.
        let empty = connect_or_launch(None, Some(OsString::new()), Some(dir.path()), false, &env)
            .unwrap_err();
        assert_eq!(empty.exit_code(), 3);
        assert!(empty.to_string().contains("set but empty"), "got: {empty}");
    }

    #[test]
    fn no_home_directory_never_starts_staged() {
        let err = connect_or_launch(
            None,
            None,
            None,
            false,
            &test_env(Some(BUNDLED_EXECUTABLE), &NeverLaunch),
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 3, "{err}");
    }

    #[test]
    fn a_launch_that_fails_is_exit_3_with_the_reason_and_the_bundle() {
        let dir = private_tempdir();
        store_grant(dir.path());
        let launcher = FakeLauncher::new(|| Err("the application cannot be opened".to_string()));

        let err = connect_or_launch(
            None,
            None,
            Some(dir.path()),
            false,
            &test_env(Some(BUNDLED_EXECUTABLE), &launcher),
        )
        .unwrap_err();
        assert_eq!(launcher.launched(), vec![PathBuf::from(BUNDLE)]);
        assert_eq!(err.exit_code(), 3);
        assert_eq!(
            err.to_string(),
            "could not start Staged at /Applications/Staged.app: the application cannot be \
             opened; start it and run the command again"
        );
    }

    #[test]
    fn a_started_staged_that_never_answers_is_exit_3_and_is_started_only_once() {
        let dir = private_tempdir();
        store_grant(dir.path());
        let launcher = FakeLauncher::new(|| Ok(()));
        let mut env = test_env(Some(BUNDLED_EXECUTABLE), &launcher);
        env.timeout = Duration::from_millis(200);

        let err = connect_or_launch(None, None, Some(dir.path()), false, &env).unwrap_err();
        assert_eq!(launcher.launched().len(), 1, "no second attempt");
        assert_eq!(err.exit_code(), 3);
        assert!(
            err.to_string().contains("did not answer within"),
            "got: {err}"
        );
    }
}
