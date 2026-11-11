//! On-disk layout of one control-server instance under
//! `~/.staged/control/<pid>/`, and the lifecycle around it.
//!
//! ```text
//! ~/.staged/control/                 0700, owned by the app's user
//! └── <pid>/                         0700, one per running instance
//!     ├── sock                       the Unix domain socket (unix only)
//!     ├── discovery.json             0600 {socket|port+token, pid, generation, protocolVersion}
//!     ├── sessions/                  0700, per-session capability files (class A grants)
//!     └── bin/stagedctl              symlink to the bundled sidecar (PATH shim)
//! ```
//!
//! Everything about an instance lives in its own directory so a stale
//! instance — a crashed app whose pid is dead — is swept with one
//! `remove_dir_all`, and our own goes the same way at exit. The root and
//! each instance directory are created `0700` and refused if they turn out
//! to be a symlink or owned by someone else (berd's rule for its discovery
//! directory): other local accounts then cannot connect to the socket, read
//! the discovery file, or plant a replacement for either.

use std::path::{Path, PathBuf};

use super::PROTOCOL_VERSION;

/// Name of the per-instance socket file.
pub(super) const SOCKET_FILE: &str = "sock";
/// Name of the per-instance discovery file.
pub(super) const DISCOVERY_FILE_NAME: &str = "discovery.json";
/// Name of the per-instance directory of per-session capability files.
pub(super) const SESSIONS_DIR: &str = "sessions";
/// Name of the per-instance PATH shim directory.
pub(super) const SHIM_DIR: &str = "bin";

/// `~/.staged/control`, the root every instance directory lives under.
pub(super) fn control_root(data_dir: &Path) -> PathBuf {
    data_dir.join("control")
}

/// `~/.staged/control/<pid>`.
pub(super) fn instance_dir_path(root: &Path, pid: u32) -> PathBuf {
    root.join(pid.to_string())
}

pub(super) fn socket_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(SOCKET_FILE)
}

pub(super) fn discovery_file_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(DISCOVERY_FILE_NAME)
}

pub(super) fn sessions_dir_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(SESSIONS_DIR)
}

pub(super) fn shim_dir_path(instance_dir: &Path) -> PathBuf {
    instance_dir.join(SHIM_DIR)
}

// =============================================================================
// Private directories and files
// =============================================================================

/// Make `path` an owner-only directory we can trust: create it `0700` if
/// missing; if present, refuse a symlink, a non-directory, or a directory
/// owned by another user, and tighten a too-permissive mode back to `0700`
/// (it is ours, so the mode is ours to fix). The refusals are what make the
/// socket and the files inside unreachable to other local accounts.
pub(super) fn ensure_private_dir(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(format!("{path:?} is a symlink; refusing to use it"));
            }
            if !meta.is_dir() {
                return Err(format!("{path:?} exists but is not a directory"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if meta.uid() != current_uid() {
                    return Err(format!(
                        "{path:?} is owned by uid {}, not the current user; refusing to use it",
                        meta.uid()
                    ));
                }
                if meta.mode() & 0o077 != 0 {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                        .map_err(|e| format!("chmod 0700 {path:?}: {e}"))?;
                }
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(path)
                .map_err(|e| format!("create {path:?}: {e}"))
        }
        Err(e) => Err(format!("inspect {path:?}: {e}")),
    }
}

/// Create a fresh, empty, owner-only instance directory, replacing whatever a
/// dead instance of the same pid left behind (pids are reused across app
/// restarts). The parent root must already have passed [`ensure_private_dir`].
pub(super) fn create_fresh_instance_dir(instance_dir: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(instance_dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove stale {instance_dir:?}: {e}")),
    }
    ensure_private_dir(instance_dir)?;
    ensure_private_dir(&sessions_dir_path(instance_dir))
}

/// Atomically write an owner-only file: temp file beside the target with
/// `0600` permissions, fsync, then rename — so a reader never sees partial
/// JSON and other users never see the contents at all.
pub(super) fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| format!("create {tmp:?}: {e}"))?;
        file.write_all(contents)
            .map_err(|e| format!("write {tmp:?}: {e}"))?;
        file.sync_all().map_err(|e| format!("fsync {tmp:?}: {e}"))?;
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {tmp:?} -> {path:?}: {e}"))
}

#[cfg(unix)]
pub(super) fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

// =============================================================================
// Discovery file
// =============================================================================

/// How a CLI reaches this instance, as recorded in the discovery file and
/// pinned by the middleware in `serve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlTransport {
    /// A Unix domain socket inside the instance directory. The directory's
    /// `0700` mode plus the peer-uid check on every connection are the
    /// authentication; no token is needed to keep other users out.
    Unix { socket: PathBuf },
    /// Loopback TCP, used only where a Unix socket is unavailable (non-unix
    /// builds, or a socket path over the platform limit). A port is reachable
    /// by every local account, so a per-start bearer stands in for the uid
    /// check; it lives only in the owner-only discovery file.
    Tcp { port: u16, token: String },
}

impl ControlTransport {
    /// Human-readable endpoint for logs.
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Unix { socket } => socket.display().to_string(),
            Self::Tcp { port, .. } => format!("127.0.0.1:{port}"),
        }
    }
}

/// The discovery file's JSON. Each key the CLI reads is spelled once here;
/// `stagedctl`'s `Discovery` struct mirrors it.
pub(super) fn discovery_payload(
    transport: &ControlTransport,
    pid: u32,
    generation: u64,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "pid": pid,
        "generation": generation,
        "protocolVersion": PROTOCOL_VERSION,
    });
    match transport {
        ControlTransport::Unix { socket } => {
            payload["socket"] = serde_json::Value::String(socket.display().to_string());
        }
        ControlTransport::Tcp { port, token } => {
            payload["port"] = serde_json::Value::from(*port);
            payload["token"] = serde_json::Value::String(token.clone());
        }
    }
    payload
}

pub(super) fn write_discovery_file(
    path: &Path,
    transport: &ControlTransport,
    pid: u32,
    generation: u64,
) -> Result<(), String> {
    let contents = serde_json::to_vec_pretty(&discovery_payload(transport, pid, generation))
        .map_err(|e| format!("serialize discovery: {e}"))?;
    write_private_file(path, &contents)
}

// =============================================================================
// Sweep
// =============================================================================

/// Remove every `control/<pid>` instance directory whose pid is dead. The
/// root is shared by every Staged instance, so live pids (and our own) are
/// left alone, as is anything whose name is not a pid.
pub(super) fn sweep_stale_instances(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(instance_dir_pid) else {
            continue;
        };
        if pid == std::process::id() || pid_is_alive(pid) {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_dir_all(&path) {
            Ok(()) => log::info!("[control_server] swept stale control instance {pid}"),
            Err(e) => log::warn!("[control_server] failed to sweep stale {path:?}: {e}"),
        }
    }
}

/// Parse the pid out of an instance directory name; `None` for anything
/// else (a `grants/` directory, say).
pub(super) fn instance_dir_pid(name: &str) -> Option<u32> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    name.parse().ok()
}

/// Remove the v1 layout's leftovers from `~/.staged` itself —
/// `control-<pid>.json`, its temp file, and `control-bin-<pid>` — for dead
/// pids. A machine that ran a v1 build keeps them forever otherwise; a v1
/// instance still running keeps its own.
pub(super) fn sweep_legacy_v1_entries(data_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(legacy_entry_pid) else {
            continue;
        };
        if pid == std::process::id() || pid_is_alive(pid) {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = result {
            log::warn!("[control_server] failed to sweep stale v1 entry {path:?}: {e}");
        } else {
            log::info!("[control_server] swept stale v1 control entry {name:?}");
        }
    }
}

/// Parse the pid out of a v1 `control-<pid>.json`, `control-<pid>.json.tmp`,
/// or `control-bin-<pid>` filename; `None` for anything else.
pub(super) fn legacy_entry_pid(filename: &str) -> Option<u32> {
    let rest = filename.strip_prefix("control-")?;
    let pid = rest
        .strip_suffix(".json")
        .or_else(|| rest.strip_suffix(".json.tmp"))
        .or_else(|| rest.strip_prefix("bin-"))?;
    pid.parse().ok()
}

/// Whether `pid` names a running process. The sweep runs on the app's setup
/// thread, so this is a `kill(pid, 0)` syscall — the probe `stagedctl` uses —
/// rather than `session_runner::is_process_alive`'s `kill -0` child process
/// per entry. `EPERM` still proves something runs under that pid; a value
/// that isn't a positive `pid_t` would address a process group, so it counts
/// as dead.
#[cfg(unix)]
pub(super) fn pid_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission checks;
    // nothing is delivered.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub(super) fn pid_is_alive(pid: u32) -> bool {
    crate::session_runner::is_process_alive(pid)
}

// =============================================================================
// PATH shim
// =============================================================================

/// Create (or refresh, after a same-pid restart of the control server) the
/// shim dir containing a `stagedctl` symlink to the bundled sidecar.
#[cfg(unix)]
pub(super) fn create_shim_dir(shim_dir: &Path, bin: &Path) -> Result<(), String> {
    std::fs::create_dir_all(shim_dir).map_err(|e| format!("create {shim_dir:?}: {e}"))?;
    let link = shim_dir.join("stagedctl");
    match std::fs::remove_file(&link) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove existing {link:?}: {e}")),
    }
    std::os::unix::fs::symlink(bin, &link).map_err(|e| format!("symlink {link:?} -> {bin:?}: {e}"))
}

#[cfg(not(unix))]
pub(super) fn create_shim_dir(_shim_dir: &Path, _bin: &Path) -> Result<(), String> {
    Err("stagedctl PATH shim is only supported on unix".to_string())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// A tempdir the private-dir rules accept: `tempfile` creates
    /// directories with the default (umask) mode, which is not `0700`.
    pub(in crate::control_server) fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        ensure_private_dir(dir.path()).unwrap();
        dir
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn ensure_private_dir_creates_an_owner_only_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = control_root(dir.path());
        ensure_private_dir(&root).unwrap();
        assert!(root.is_dir());
        #[cfg(unix)]
        assert_eq!(mode_of(&root), 0o700);
    }

    #[test]
    #[cfg(unix)]
    fn ensure_private_dir_tightens_a_permissive_directory_we_own() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = control_root(dir.path());
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&root).unwrap();
        assert_eq!(mode_of(&root), 0o700);
    }

    #[test]
    #[cfg(unix)]
    fn ensure_private_dir_refuses_a_symlink_and_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let link = dir.path().join("control");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let err = ensure_private_dir(&link).unwrap_err();
        assert!(err.contains("symlink"), "got: {err}");

        let file = dir.path().join("control-file");
        std::fs::write(&file, "").unwrap();
        let err = ensure_private_dir(&file).unwrap_err();
        assert!(err.contains("not a directory"), "got: {err}");
    }

    #[test]
    fn create_fresh_instance_dir_replaces_a_dead_predecessor() {
        let root = private_tempdir();
        let instance = instance_dir_path(root.path(), 1234);
        std::fs::create_dir_all(instance.join("sessions")).unwrap();
        std::fs::write(instance.join("sessions").join("old.json"), "{}").unwrap();
        std::fs::write(discovery_file_path(&instance), "{}").unwrap();

        create_fresh_instance_dir(&instance).unwrap();

        assert!(instance.is_dir());
        assert!(sessions_dir_path(&instance).is_dir());
        assert!(!discovery_file_path(&instance).exists());
        assert!(!instance.join("sessions").join("old.json").exists());
        #[cfg(unix)]
        {
            assert_eq!(mode_of(&instance), 0o700);
            assert_eq!(mode_of(&sessions_dir_path(&instance)), 0o700);
        }
    }

    #[test]
    fn discovery_file_is_written_atomically_with_owner_only_perms() {
        let root = private_tempdir();
        let instance = instance_dir_path(root.path(), 1234);
        create_fresh_instance_dir(&instance).unwrap();
        let path = discovery_file_path(&instance);
        let transport = ControlTransport::Unix {
            socket: socket_path(&instance),
        };

        write_discovery_file(&path, &transport, 1234, 7).unwrap();

        let contents: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            contents["socket"],
            socket_path(&instance).display().to_string()
        );
        assert!(contents.get("port").is_none());
        assert!(contents.get("token").is_none());
        assert_eq!(contents["pid"], 1234);
        assert_eq!(contents["generation"], 7);
        assert_eq!(contents["protocolVersion"], PROTOCOL_VERSION);

        // No temp file left behind.
        assert!(!instance.join("discovery.json.tmp").exists());
        #[cfg(unix)]
        assert_eq!(mode_of(&path), 0o600, "discovery file must be 0600");
    }

    #[test]
    fn tcp_fallback_discovery_carries_the_port_and_token() {
        let root = private_tempdir();
        let instance = instance_dir_path(root.path(), 1234);
        create_fresh_instance_dir(&instance).unwrap();
        let path = discovery_file_path(&instance);
        let transport = ControlTransport::Tcp {
            port: 4242,
            token: "ab".repeat(32),
        };

        write_discovery_file(&path, &transport, 1234, 1).unwrap();
        // A rewrite (same pid, new generation) replaces the contents.
        let transport = ControlTransport::Tcp {
            port: 5353,
            token: "cd".repeat(32),
        };
        write_discovery_file(&path, &transport, 1234, 2).unwrap();

        let contents: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(contents.get("socket").is_none());
        assert_eq!(contents["port"], 5353);
        assert_eq!(contents["token"], "cd".repeat(32));
        assert_eq!(contents["generation"], 2);
    }

    #[test]
    fn sweep_removes_dead_pid_instances_and_keeps_live_and_foreign_ones() {
        let root = private_tempdir();

        // A pid that cannot exist (beyond macOS/Linux pid ranges) — dead.
        let dead = instance_dir_path(root.path(), 999_999_999);
        std::fs::create_dir_all(dead.join("sessions")).unwrap();
        std::fs::write(dead.join("sock"), "").unwrap();
        std::fs::write(dead.join("sessions").join("s.json"), "{}").unwrap();

        // Our own pid — alive.
        let alive = instance_dir_path(root.path(), std::process::id());
        std::fs::create_dir_all(&alive).unwrap();
        // pid 1 (init/launchd): kill(1, 0) yields EPERM, which counts as alive.
        let other = instance_dir_path(root.path(), 1);
        std::fs::create_dir_all(&other).unwrap();

        // Non-pid entries are never touched.
        let grants = root.path().join("grants");
        std::fs::create_dir_all(&grants).unwrap();
        let stray = root.path().join("notes.txt");
        std::fs::write(&stray, "x").unwrap();

        sweep_stale_instances(root.path());

        assert!(!dead.exists(), "dead-pid instance dir should be swept");
        assert!(alive.exists(), "live-pid instance dir must survive");
        assert!(other.exists(), "another live instance must survive");
        assert!(grants.exists(), "non-pid directories must survive");
        assert!(stray.exists(), "unrelated files must survive");
    }

    #[test]
    fn legacy_v1_sweep_removes_dead_pid_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        let dead = dir.path().join("control-999999999.json");
        std::fs::write(&dead, "{}").unwrap();
        let dead_tmp = dir.path().join("control-999999998.json.tmp");
        std::fs::write(&dead_tmp, "{").unwrap();
        let dead_shim = dir.path().join("control-bin-999999997");
        std::fs::create_dir_all(&dead_shim).unwrap();
        std::fs::write(dead_shim.join("stagedctl"), "").unwrap();
        let alive = dir
            .path()
            .join(format!("control-{}.json", std::process::id()));
        std::fs::write(&alive, "{}").unwrap();
        let foreign = dir.path().join("data.db");
        std::fs::write(&foreign, "not json").unwrap();
        let v2_root = control_root(dir.path());
        std::fs::create_dir_all(&v2_root).unwrap();

        sweep_legacy_v1_entries(dir.path());

        assert!(!dead.exists());
        assert!(!dead_tmp.exists());
        assert!(!dead_shim.exists());
        assert!(alive.exists(), "a running v1 instance keeps its file");
        assert!(foreign.exists());
        assert!(v2_root.exists(), "the v2 root is not a v1 entry");
    }

    #[test]
    fn instance_dir_pid_parses_only_bare_pids() {
        assert_eq!(instance_dir_pid("123"), Some(123));
        assert_eq!(instance_dir_pid("0"), Some(0));
        assert_eq!(instance_dir_pid(""), None);
        assert_eq!(instance_dir_pid("grants"), None);
        assert_eq!(instance_dir_pid("123.json"), None);
        assert_eq!(instance_dir_pid("-1"), None);
        assert_eq!(instance_dir_pid("99999999999"), None, "overflows u32");
    }

    #[test]
    fn legacy_entry_pid_parses_only_v1_control_entry_names() {
        assert_eq!(legacy_entry_pid("control-123.json"), Some(123));
        assert_eq!(legacy_entry_pid("control-123.json.tmp"), Some(123));
        assert_eq!(legacy_entry_pid("control-bin-123"), Some(123));
        assert_eq!(legacy_entry_pid("control-abc.json"), None);
        assert_eq!(legacy_entry_pid("control-.json"), None);
        assert_eq!(legacy_entry_pid("control-bin-"), None);
        assert_eq!(legacy_entry_pid("control"), None);
        assert_eq!(legacy_entry_pid("preferences.json"), None);
        assert_eq!(legacy_entry_pid("data.db"), None);
    }

    #[test]
    #[cfg(unix)]
    fn pid_is_alive_probes_without_a_child_process() {
        assert!(pid_is_alive(std::process::id()));
        assert!(!pid_is_alive(999_999_999), "beyond any pid range");
        // Not positive `pid_t`s: kill() would address a process group.
        assert!(!pid_is_alive(0));
        assert!(!pid_is_alive(u32::MAX));
    }

    #[test]
    #[cfg(unix)]
    fn shim_dir_holds_a_stagedctl_symlink_to_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("stagedctl-sidecar");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();

        let shim_dir = shim_dir_path(&instance_dir_path(dir.path(), 1234));
        create_shim_dir(&shim_dir, &bin).unwrap();

        let link = shim_dir.join("stagedctl");
        assert_eq!(std::fs::read_link(&link).unwrap(), bin);
    }

    #[test]
    #[cfg(unix)]
    fn shim_dir_refresh_replaces_an_existing_symlink() {
        // Pids are reused across app restarts of the same process image; a
        // stale symlink from an earlier start of this pid must be replaced,
        // not error the setup.
        let dir = tempfile::tempdir().unwrap();
        let old_bin = dir.path().join("old-sidecar");
        let new_bin = dir.path().join("new-sidecar");
        std::fs::write(&old_bin, "").unwrap();
        std::fs::write(&new_bin, "").unwrap();

        let shim_dir = shim_dir_path(&instance_dir_path(dir.path(), 1234));
        create_shim_dir(&shim_dir, &old_bin).unwrap();
        create_shim_dir(&shim_dir, &new_bin).unwrap();

        let link = shim_dir.join("stagedctl");
        assert_eq!(std::fs::read_link(&link).unwrap(), new_bin);
    }
}
