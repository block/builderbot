//! Moving files to and from a workspace.
//!
//! `ws exec` has no stdin and a 300 s cap, so files travel over `ws ssh`
//! instead: byte-exact in both directions with no size limit. `ws ssh` dials
//! the workstation's internal address, which can fail off the corporate
//! network, so `ws upload` / `ws download` are the fallback. They go through
//! the orchestrator, which rejects messages above 64 MiB, so a file larger
//! than [`MAX_PART_BYTES`] moves in parts that are joined at the far end.
//!
//! Every function here blocks, and every timeout scales with the payload (see
//! [`transfer_timeout`]). The decisions — which route, how many parts, what to
//! clean up — run over [`WsTransport`], so tests can drive them against a
//! directory standing in for the workstation.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use super::{is_auth_error, shell_quote, sq_binary, strip_ansi_escape_sequences, BloxError};

/// Largest part `ws upload` / `ws download` are asked to carry, under the
/// orchestrator's 64 MiB message cap with room for framing. A whole number of
/// MiB, because the remote split counts in 1 MiB blocks.
pub const MAX_PART_BYTES: u64 = 60 * MIB;

const MIB: u64 = 1024 * 1024;

/// Fixed allowance for connection setup in every transfer.
const TRANSFER_BASE_TIMEOUT_SECS: u64 = 60;

/// Slowest throughput a transfer is allowed before it is treated as hung.
/// Measured routes run at 3–8 MB/s.
const TRANSFER_MIN_BYTES_PER_SEC: u64 = 1_000_000;

/// How long moving `bytes` may take before it is killed: 60 s plus one
/// second per megabyte.
pub fn transfer_timeout(bytes: u64) -> Duration {
    Duration::from_secs(TRANSFER_BASE_TIMEOUT_SECS + bytes / TRANSFER_MIN_BYTES_PER_SEC)
}

/// Args for `sq blox ws ssh <name> -- <remote_cmd>`. The remote login shell
/// parses `remote_cmd`, so callers quote what it contains.
pub fn ws_ssh_args(name: &str, remote_cmd: &str) -> Vec<OsString> {
    ["blox", "ws", "ssh", name, "--", remote_cmd]
        .into_iter()
        .map(OsString::from)
        .collect()
}

/// Args for `sq blox ws upload --dest=<dest> <name> <local>`. `dest` must sit
/// under the workstation's home directory, in a directory that exists.
pub fn ws_upload_args(name: &str, local: &Path, dest: &str) -> Vec<OsString> {
    vec![
        "blox".into(),
        "ws".into(),
        "upload".into(),
        format!("--dest={dest}").into(),
        name.into(),
        local.into(),
    ]
}

/// Args for `sq blox ws download --output=<out> <name> <remote>`.
pub fn ws_download_args(name: &str, remote: &str, out: &Path) -> Vec<OsString> {
    let mut output = OsString::from("--output=");
    output.push(out);
    vec![
        "blox".into(),
        "ws".into(),
        "download".into(),
        output,
        name.into(),
        remote.into(),
    ]
}

/// Run `remote_cmd` on the workstation with `stdin_path` as its stdin, and
/// return its stdout.
pub fn ws_ssh_put(
    name: &str,
    remote_cmd: &str,
    stdin_path: &Path,
    timeout: Duration,
) -> Result<Vec<u8>, BloxError> {
    let stdin = File::open(stdin_path).map_err(|e| io_error("open", stdin_path, e))?;
    run_sq(&ws_ssh_args(name, remote_cmd), stdin.into(), None, timeout)
}

/// Run `remote_cmd` on the workstation and write its stdout to `stdout_path`.
pub fn ws_ssh_get(
    name: &str,
    remote_cmd: &str,
    stdout_path: &Path,
    timeout: Duration,
) -> Result<(), BloxError> {
    let stdout = File::create(stdout_path).map_err(|e| io_error("create", stdout_path, e))?;
    run_sq(
        &ws_ssh_args(name, remote_cmd),
        Stdio::null(),
        Some(stdout),
        timeout,
    )?;
    Ok(())
}

/// Upload one file as-is (no extraction) to `dest` on the workstation.
pub fn ws_upload(name: &str, local: &Path, dest: &str, timeout: Duration) -> Result<(), BloxError> {
    run_sq(
        &ws_upload_args(name, local, dest),
        Stdio::null(),
        None,
        timeout,
    )?;
    Ok(())
}

/// Download one file from the workstation to `out`.
pub fn ws_download(
    name: &str,
    remote: &str,
    out: &Path,
    timeout: Duration,
) -> Result<(), BloxError> {
    run_sq(
        &ws_download_args(name, remote, out),
        Stdio::null(),
        None,
        timeout,
    )?;
    Ok(())
}

/// How a file reached the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRoute {
    /// Streamed over `ws ssh`.
    Ssh,
    /// Moved with `ws upload` / `ws download`, in this many parts.
    Parts(usize),
}

/// The workstation operations a file transfer is built from. [`SqWorkspace`]
/// runs them through `sq`; tests stand a directory in for the workstation.
pub trait WsTransport {
    /// Run `remote_cmd` with `stdin` as its stdin; return its stdout.
    fn ssh_put(
        &self,
        remote_cmd: &str,
        stdin: &Path,
        timeout: Duration,
    ) -> Result<Vec<u8>, BloxError>;
    /// Run `remote_cmd`, writing its stdout to `out`.
    fn ssh_get(&self, remote_cmd: &str, out: &Path, timeout: Duration) -> Result<(), BloxError>;
    fn upload(&self, local: &Path, dest: &str, timeout: Duration) -> Result<(), BloxError>;
    fn download(&self, remote: &str, out: &Path, timeout: Duration) -> Result<(), BloxError>;
    /// Run argv without a shell in between; return stdout.
    fn exec(&self, args: &[&str]) -> Result<String, BloxError>;
}

/// [`WsTransport`] for a real workstation.
pub struct SqWorkspace<'a> {
    pub name: &'a str,
}

impl WsTransport for SqWorkspace<'_> {
    fn ssh_put(
        &self,
        remote_cmd: &str,
        stdin: &Path,
        timeout: Duration,
    ) -> Result<Vec<u8>, BloxError> {
        ws_ssh_put(self.name, remote_cmd, stdin, timeout)
    }

    fn ssh_get(&self, remote_cmd: &str, out: &Path, timeout: Duration) -> Result<(), BloxError> {
        ws_ssh_get(self.name, remote_cmd, out, timeout)
    }

    fn upload(&self, local: &Path, dest: &str, timeout: Duration) -> Result<(), BloxError> {
        ws_upload(self.name, local, dest, timeout)
    }

    fn download(&self, remote: &str, out: &Path, timeout: Duration) -> Result<(), BloxError> {
        ws_download(self.name, remote, out, timeout)
    }

    fn exec(&self, args: &[&str]) -> Result<String, BloxError> {
        super::ws_exec(self.name, args)
    }
}

/// Copy `local` to `remote_path` (absolute, under the workstation's home) on
/// workspace `name`, creating its directory. See [`put_file_with`].
pub fn ws_put_file(name: &str, local: &Path, remote_path: &str) -> Result<FileRoute, BloxError> {
    put_file_with(&SqWorkspace { name }, local, remote_path, MAX_PART_BYTES)
}

/// Copy `remote_path` on workspace `name` to `local`. See [`get_file_with`].
pub fn ws_get_file(
    name: &str,
    remote_path: &str,
    local: &Path,
    size: u64,
) -> Result<FileRoute, BloxError> {
    get_file_with(
        &SqWorkspace { name },
        remote_path,
        local,
        size,
        MAX_PART_BYTES,
    )
}

/// Stream `local` to `remote_path` over ssh, falling back to
/// [`upload_in_parts`] (parts of at most `part_bytes`) when ssh fails or
/// delivers the wrong size.
///
/// The remote file only ever appears whole: ssh writes `<path>.part` and
/// renames it, and the fallback joins its parts the same way.
pub fn put_file_with(
    t: &dyn WsTransport,
    local: &Path,
    remote_path: &str,
    part_bytes: u64,
) -> Result<FileRoute, BloxError> {
    let size = file_len(local)?;
    let dir = remote_parent(remote_path)?;
    let path = shell_quote(remote_path);
    let part = shell_quote(&format!("{remote_path}.part"));
    let cmd = format!(
        "mkdir -p -- {dir} && cat > {part} && mv -f -- {part} {path} && wc -c < {path}",
        dir = shell_quote(dir),
    );
    let started = Instant::now();
    match t.ssh_put(&cmd, local, transfer_timeout(size)) {
        Ok(stdout) if parse_size(&stdout) == Some(size) => {
            log::info!(
                "[blox-cli] ssh upload complete: path={remote_path} bytes={size} elapsed_ms={}",
                started.elapsed().as_millis()
            );
            return Ok(FileRoute::Ssh);
        }
        Ok(stdout) => log::warn!(
            "[blox-cli] ssh upload of {remote_path} landed {:?} bytes, expected {size}; retrying in parts",
            String::from_utf8_lossy(&stdout).trim()
        ),
        Err(e) if is_fatal(&e) => return Err(e),
        Err(e) => log::warn!("[blox-cli] ssh upload of {remote_path} failed: {e}; retrying in parts"),
    }
    upload_in_parts(t, local, remote_path, part_bytes).map(FileRoute::Parts)
}

/// Stream `remote_path` to `local` over ssh, falling back to
/// [`download_in_parts`] (parts of at most `part_bytes`) when ssh fails or
/// delivers the wrong size. `size` is what the caller expects: it sizes the
/// timeouts and plans the parts.
///
/// `local` only ever appears whole: both routes write beside it and rename.
pub fn get_file_with(
    t: &dyn WsTransport,
    remote_path: &str,
    local: &Path,
    size: u64,
    part_bytes: u64,
) -> Result<FileRoute, BloxError> {
    let partial = sibling(local, "part");
    let cmd = format!("cat -- {}", shell_quote(remote_path));
    let started = Instant::now();
    match t.ssh_get(&cmd, &partial, transfer_timeout(size)) {
        Ok(()) if file_len(&partial).ok() == Some(size) => {
            rename(&partial, local)?;
            log::info!(
                "[blox-cli] ssh download complete: path={remote_path} bytes={size} elapsed_ms={}",
                started.elapsed().as_millis()
            );
            return Ok(FileRoute::Ssh);
        }
        Ok(()) => log::warn!(
            "[blox-cli] ssh download of {remote_path} delivered {:?} bytes, expected {size}; retrying in parts",
            file_len(&partial).ok()
        ),
        Err(e) if is_fatal(&e) => {
            let _ = fs::remove_file(&partial);
            return Err(e);
        }
        Err(e) => {
            log::warn!("[blox-cli] ssh download of {remote_path} failed: {e}; retrying in parts")
        }
    }
    let _ = fs::remove_file(&partial);
    download_in_parts(t, remote_path, size, local, part_bytes).map(FileRoute::Parts)
}

/// Joins uploaded parts: `$1` is the destination, the rest are the parts in
/// order. Prints the joined size.
const JOIN_PARTS_SCRIPT: &str = "dest=$1; shift; \
cat -- \"$@\" > \"$dest.part\" && mv -f -- \"$dest.part\" \"$dest\" && rm -f -- \"$@\" && wc -c < \"$dest\"";

/// Cuts `$1` into `$3` parts of `$2` MiB named `$1.part<i>`, with POSIX `dd`
/// so it runs under dash.
const SPLIT_PARTS_SCRIPT: &str = "f=$1 mib=$2 n=$3 i=0; \
while [ \"$i\" -lt \"$n\" ]; do \
dd if=\"$f\" of=\"$f.part$i\" bs=1048576 skip=$((i * mib)) count=\"$mib\" 2>/dev/null || exit 1; \
i=$((i + 1)); done";

const REMOVE_FILES_SCRIPT: &str = "rm -f -- \"$@\"";

/// Upload `local` to `dest` with `ws upload`, in parts of at most
/// `part_bytes`, and join them at the far end. Returns the number of parts.
///
/// Parts are cut one at a time beside `local`, so the local overhead is one
/// part. Remote parts are removed whether or not the join succeeds.
pub fn upload_in_parts(
    t: &dyn WsTransport,
    local: &Path,
    dest: &str,
    part_bytes: u64,
) -> Result<usize, BloxError> {
    let size = file_len(local)?;
    let count = part_count(size, part_bytes);
    t.exec(&["mkdir", "-p", "--", remote_parent(dest)?])?;

    let staged = sibling(local, "upload-part");
    let mut source = File::open(local).map_err(|e| io_error("open", local, e))?;
    let mut remote_parts = Vec::with_capacity(count);
    let uploaded = (0..count).try_for_each(|i| {
        let len = {
            let mut piece = File::create(&staged).map_err(|e| io_error("create", &staged, e))?;
            io::copy(&mut (&mut source).take(part_bytes), &mut piece)
                .map_err(|e| io_error("write", &staged, e))?
        };
        let remote_part = format!("{dest}.part{i}");
        t.upload(&staged, &remote_part, transfer_timeout(len))?;
        remote_parts.push(remote_part);
        Ok(())
    });
    let _ = fs::remove_file(&staged);
    if let Err(e) = uploaded {
        remove_remote(t, &remote_parts);
        return Err(e);
    }

    let mut args = vec!["sh", "-c", JOIN_PARTS_SCRIPT, "staged-upload", dest];
    args.extend(remote_parts.iter().map(String::as_str));
    let joined = t
        .exec(&args)
        .inspect_err(|_| remove_remote(t, &remote_parts))?;
    match parse_size(joined.as_bytes()) {
        Some(n) if n == size => Ok(count),
        other => Err(BloxError::CommandFailed(format!(
            "upload of {dest} joined to {other:?} bytes, expected {size}"
        ))),
    }
}

/// Download `remote` (expected to be `size` bytes) to `out` with
/// `ws download`, in parts of at most `part_bytes` (a whole number of MiB)
/// cut at the far end. Returns the number of parts.
pub fn download_in_parts(
    t: &dyn WsTransport,
    remote: &str,
    size: u64,
    out: &Path,
    part_bytes: u64,
) -> Result<usize, BloxError> {
    let part_bytes = (part_bytes / MIB).max(1) * MIB;
    let count = part_count(size, part_bytes);
    let partial = sibling(out, "part");
    let result = if count == 1 {
        t.download(remote, &partial, transfer_timeout(size))
    } else {
        download_parts(t, remote, count, part_bytes, &partial)
    };
    if let Err(e) = result {
        let _ = fs::remove_file(&partial);
        return Err(e);
    }
    match file_len(&partial) {
        Ok(n) if n == size => {
            rename(&partial, out)?;
            Ok(count)
        }
        other => {
            let _ = fs::remove_file(&partial);
            Err(BloxError::CommandFailed(format!(
                "download of {remote} delivered {:?} bytes, expected {size}",
                other.ok()
            )))
        }
    }
}

fn download_parts(
    t: &dyn WsTransport,
    remote: &str,
    count: usize,
    part_bytes: u64,
    partial: &Path,
) -> Result<(), BloxError> {
    let remote_parts: Vec<String> = (0..count).map(|i| format!("{remote}.part{i}")).collect();
    let mib = (part_bytes / MIB).to_string();
    let count_arg = count.to_string();
    let split = t.exec(&[
        "sh",
        "-c",
        SPLIT_PARTS_SCRIPT,
        "staged-download",
        remote,
        &mib,
        &count_arg,
    ]);
    let result = split.and_then(|_| {
        let mut joined = File::create(partial).map_err(|e| io_error("create", partial, e))?;
        let piece = sibling(partial, "piece");
        let appended = remote_parts.iter().try_for_each(|remote_part| {
            t.download(remote_part, &piece, transfer_timeout(part_bytes))?;
            let mut src = File::open(&piece).map_err(|e| io_error("open", &piece, e))?;
            io::copy(&mut src, &mut joined).map_err(|e| io_error("write", partial, e))?;
            Ok(())
        });
        let _ = fs::remove_file(&piece);
        appended
    });
    remove_remote(t, &remote_parts);
    result
}

/// Best-effort removal of scratch files on the workstation.
fn remove_remote(t: &dyn WsTransport, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let mut args = vec!["sh", "-c", REMOVE_FILES_SCRIPT, "staged-cleanup"];
    args.extend(paths.iter().map(String::as_str));
    if let Err(e) = t.exec(&args) {
        log::warn!("[blox-cli] could not remove transfer parts on the workstation: {e}");
    }
}

/// Errors no other route can get past: retrying without `sq`, or without a
/// login, fails the same way.
fn is_fatal(e: &BloxError) -> bool {
    matches!(e, BloxError::NotFound | BloxError::NotAuthenticated)
}

fn part_count(size: u64, part_bytes: u64) -> usize {
    size.div_ceil(part_bytes.max(1)).max(1) as usize
}

fn remote_parent(path: &str) -> Result<&str, BloxError> {
    match path.rsplit_once('/') {
        Some((dir, file)) if dir.starts_with('/') && !file.is_empty() => Ok(dir),
        _ => Err(BloxError::CommandFailed(format!(
            "workstation path must be absolute: {path}"
        ))),
    }
}

/// `<path>.<suffix>`, beside `path`.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn parse_size(stdout: &[u8]) -> Option<u64> {
    String::from_utf8_lossy(stdout).trim().parse().ok()
}

fn file_len(path: &Path) -> Result<u64, BloxError> {
    fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| io_error("stat", path, e))
}

fn rename(from: &Path, to: &Path) -> Result<(), BloxError> {
    fs::rename(from, to).map_err(|e| io_error("rename", from, e))
}

fn io_error(op: &str, path: &Path, e: io::Error) -> BloxError {
    BloxError::CommandFailed(format!("cannot {op} {}: {e}", path.display()))
}

/// Run `sq <args>` with the given stdin, and stdout either into `stdout` or
/// captured and returned. Stderr is captured for the error message.
fn run_sq(
    args: &[OsString],
    stdin: Stdio,
    stdout: Option<File>,
    timeout: Duration,
) -> Result<Vec<u8>, BloxError> {
    let mut command = Command::new(sq_binary()?);
    command.args(args).stdin(stdin).stderr(Stdio::piped());
    let capture = stdout.is_none();
    match stdout {
        Some(file) => command.stdout(file),
        None => command.stdout(Stdio::piped()),
    };
    let mut child = command
        .spawn()
        .map_err(|e| BloxError::CommandFailed(e.to_string()))?;

    let stdout_reader = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_reader = child.stderr.take().map(|mut err| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            buf
        })
    });
    let join = |reader: Option<std::thread::JoinHandle<Vec<u8>>>| {
        reader
            .map(|r| r.join().unwrap_or_default())
            .unwrap_or_default()
    };

    let status = match child
        .wait_timeout(timeout)
        .map_err(|e| BloxError::CommandFailed(e.to_string()))?
    {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            join(stdout_reader);
            join(stderr_reader);
            return Err(BloxError::Timeout(timeout.as_secs()));
        }
    };
    let out = join(stdout_reader);
    let err = join(stderr_reader);
    if !status.success() {
        let stderr = strip_ansi_escape_sequences(&String::from_utf8_lossy(&err));
        if is_auth_error(&stderr) {
            return Err(BloxError::NotAuthenticated);
        }
        return Err(BloxError::CommandFailed(stderr));
    }
    Ok(if capture { out } else { Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "blox-cli-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A workstation that is a local directory: "remote" paths are real
    /// paths, remote commands run under the local `sh`, and `ws upload` /
    /// `ws download` are copies that enforce a message cap.
    struct FakeWorkstation {
        ssh_works: bool,
        message_cap: u64,
        uploads: Cell<usize>,
        downloads: Cell<usize>,
        execs: RefCell<Vec<String>>,
    }

    impl FakeWorkstation {
        fn new(ssh_works: bool, message_cap: u64) -> Self {
            Self {
                ssh_works,
                message_cap,
                uploads: Cell::new(0),
                downloads: Cell::new(0),
                execs: RefCell::new(Vec::new()),
            }
        }

        fn ssh_unreachable() -> BloxError {
            BloxError::CommandFailed("dial tcp 10.0.0.1:22: i/o timeout".into())
        }

        fn check_cap(&self, path: &Path) -> Result<(), BloxError> {
            let len = file_len(path)?;
            if len > self.message_cap {
                return Err(BloxError::CommandFailed(format!(
                    "message size {len} is larger than configured max {}",
                    self.message_cap
                )));
            }
            Ok(())
        }
    }

    fn sh(cmd: &str, stdin: Stdio, stdout: Stdio) -> Result<Vec<u8>, BloxError> {
        let output = Command::new("/bin/sh")
            .args(["-c", cmd])
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        if !output.status.success() {
            return Err(BloxError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        Ok(output.stdout)
    }

    impl WsTransport for FakeWorkstation {
        fn ssh_put(&self, cmd: &str, stdin: &Path, _: Duration) -> Result<Vec<u8>, BloxError> {
            if !self.ssh_works {
                return Err(Self::ssh_unreachable());
            }
            sh(cmd, File::open(stdin).unwrap().into(), Stdio::piped())
        }

        fn ssh_get(&self, cmd: &str, out: &Path, _: Duration) -> Result<(), BloxError> {
            if !self.ssh_works {
                return Err(Self::ssh_unreachable());
            }
            sh(cmd, Stdio::null(), File::create(out).unwrap().into()).map(|_| ())
        }

        fn upload(&self, local: &Path, dest: &str, _: Duration) -> Result<(), BloxError> {
            self.check_cap(local)?;
            self.uploads.set(self.uploads.get() + 1);
            fs::copy(local, dest)
                .map(|_| ())
                .map_err(|e| io_error("copy", local, e))
        }

        fn download(&self, remote: &str, out: &Path, _: Duration) -> Result<(), BloxError> {
            self.check_cap(Path::new(remote))?;
            self.downloads.set(self.downloads.get() + 1);
            fs::copy(remote, out)
                .map(|_| ())
                .map_err(|e| io_error("copy", out, e))
        }

        fn exec(&self, args: &[&str]) -> Result<String, BloxError> {
            self.execs.borrow_mut().push(args.join(" "));
            let output = Command::new(args[0]).args(&args[1..]).output().unwrap();
            if !output.status.success() {
                return Err(BloxError::CommandFailed(
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                ));
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
    }

    /// Bytes that differ at every offset, so a misordered or truncated
    /// join cannot compare equal.
    fn payload(len: usize) -> Vec<u8> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    fn write(path: &Path, bytes: &[u8]) {
        File::create(path).unwrap().write_all(bytes).unwrap();
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".part") || n.contains(".piece"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn timeout_allows_a_minute_plus_a_second_per_megabyte() {
        assert_eq!(transfer_timeout(0), Duration::from_secs(60));
        assert_eq!(transfer_timeout(999_999), Duration::from_secs(60));
        assert_eq!(transfer_timeout(80_000_000), Duration::from_secs(140));
    }

    #[test]
    fn builds_the_sq_argument_lists() {
        assert_eq!(
            ws_ssh_args("stg-x", "cat -- '/home/bloxer/a b'"),
            [
                "blox",
                "ws",
                "ssh",
                "stg-x",
                "--",
                "cat -- '/home/bloxer/a b'"
            ]
            .map(OsString::from)
            .to_vec()
        );
        assert_eq!(
            ws_upload_args(
                "stg-x",
                Path::new("/tmp/b.bundle"),
                "/home/bloxer/t/b.bundle"
            ),
            [
                "blox",
                "ws",
                "upload",
                "--dest=/home/bloxer/t/b.bundle",
                "stg-x",
                "/tmp/b.bundle"
            ]
            .map(OsString::from)
            .to_vec()
        );
        assert_eq!(
            ws_download_args(
                "stg-x",
                "/home/bloxer/t/b.bundle",
                Path::new("/tmp/b.bundle")
            ),
            [
                "blox",
                "ws",
                "download",
                "--output=/tmp/b.bundle",
                "stg-x",
                "/home/bloxer/t/b.bundle"
            ]
            .map(OsString::from)
            .to_vec()
        );
    }

    #[test]
    fn counts_parts_and_never_plans_zero() {
        assert_eq!(part_count(0, MIB), 1);
        assert_eq!(part_count(MIB, MIB), 1);
        assert_eq!(part_count(MIB + 1, MIB), 2);
        assert_eq!(part_count(150 * MIB, MAX_PART_BYTES), 3);
    }

    #[test]
    fn rejects_relative_workstation_paths() {
        assert!(remote_parent("file.bundle").is_err());
        assert!(remote_parent("t/file.bundle").is_err());
        assert!(remote_parent("/home/bloxer/").is_err());
        assert_eq!(remote_parent("/home/bloxer/t/f").unwrap(), "/home/bloxer/t");
    }

    #[test]
    fn put_streams_over_ssh_into_a_new_directory() {
        let local = Scratch::new("put-local");
        let remote = Scratch::new("put-remote");
        let src = local.0.join("b.bundle");
        let bytes = payload(300_000);
        write(&src, &bytes);
        let dest = remote.0.join("deep/dir/it's here.bundle");
        let ws = FakeWorkstation::new(true, MIB);

        let route = put_file_with(&ws, &src, dest.to_str().unwrap(), MIB).unwrap();

        assert_eq!(route, FileRoute::Ssh);
        assert_eq!(fs::read(&dest).unwrap(), bytes);
        assert_eq!(ws.uploads.get(), 0);
        assert!(leftovers(dest.parent().unwrap()).is_empty());
    }

    /// Off the corporate network ssh cannot dial the workstation; the upload
    /// falls back to parts under the message cap and joins them in order.
    #[test]
    fn put_falls_back_to_parts_when_ssh_cannot_connect() {
        let local = Scratch::new("parts-local");
        let remote = Scratch::new("parts-remote");
        let src = local.0.join("b.bundle");
        let bytes = payload((2 * MIB + MIB / 2) as usize);
        write(&src, &bytes);
        let dest = remote.0.join("t/b.bundle");
        let ws = FakeWorkstation::new(false, MIB);

        let route = put_file_with(&ws, &src, dest.to_str().unwrap(), MIB).unwrap();

        assert_eq!(route, FileRoute::Parts(3));
        assert_eq!(ws.uploads.get(), 3);
        assert_eq!(fs::read(&dest).unwrap(), bytes);
        assert!(leftovers(dest.parent().unwrap()).is_empty());
        assert!(leftovers(&local.0).is_empty());
    }

    #[test]
    fn put_of_an_empty_file_still_creates_it() {
        let local = Scratch::new("empty-local");
        let remote = Scratch::new("empty-remote");
        let src = local.0.join("empty");
        write(&src, b"");
        let ws = FakeWorkstation::new(false, MIB);
        let dest = remote.0.join("empty");

        assert_eq!(
            upload_in_parts(&ws, &src, dest.to_str().unwrap(), MIB).unwrap(),
            1
        );
        assert_eq!(fs::read(&dest).unwrap(), b"");
    }

    #[test]
    fn get_streams_over_ssh() {
        let local = Scratch::new("get-local");
        let remote = Scratch::new("get-remote");
        let src = remote.0.join("b.bundle");
        let bytes = payload(200_000);
        write(&src, &bytes);
        let out = local.0.join("b.bundle");
        let ws = FakeWorkstation::new(true, MIB);

        let route =
            get_file_with(&ws, src.to_str().unwrap(), &out, bytes.len() as u64, MIB).unwrap();

        assert_eq!(route, FileRoute::Ssh);
        assert_eq!(fs::read(&out).unwrap(), bytes);
        assert!(leftovers(&local.0).is_empty());
    }

    #[test]
    fn get_falls_back_to_parts_cut_on_the_workstation() {
        let local = Scratch::new("dl-local");
        let remote = Scratch::new("dl-remote");
        let src = remote.0.join("b.bundle");
        let bytes = payload((2 * MIB + 17) as usize);
        write(&src, &bytes);
        let out = local.0.join("b.bundle");
        let ws = FakeWorkstation::new(false, MIB);

        let route =
            get_file_with(&ws, src.to_str().unwrap(), &out, bytes.len() as u64, MIB).unwrap();

        assert_eq!(route, FileRoute::Parts(3));
        assert_eq!(ws.downloads.get(), 3);
        assert_eq!(fs::read(&out).unwrap(), bytes);
        assert!(leftovers(&local.0).is_empty());
        assert!(leftovers(&remote.0).is_empty(), "remote parts are removed");
    }

    #[test]
    fn get_rejects_a_short_download_and_leaves_nothing_behind() {
        let local = Scratch::new("short-local");
        let remote = Scratch::new("short-remote");
        let src = remote.0.join("b.bundle");
        write(&src, &payload(1000));
        let out = local.0.join("b.bundle");
        let ws = FakeWorkstation::new(true, MIB);

        let err = get_file_with(&ws, src.to_str().unwrap(), &out, 2000, MIB).unwrap_err();

        assert!(err.to_string().contains("expected 2000"), "{err}");
        assert!(!out.exists());
        assert!(leftovers(&local.0).is_empty());
    }

    #[test]
    fn failed_part_uploads_clean_up_on_the_workstation() {
        let local = Scratch::new("fail-local");
        let remote = Scratch::new("fail-remote");
        let src = local.0.join("b.bundle");
        write(&src, &payload((2 * MIB) as usize));
        let dest = remote.0.join("t/b.bundle");
        // Parts of 2 MiB against a 1 MiB cap: the first upload is refused.
        let ws = FakeWorkstation::new(false, MIB);

        assert!(upload_in_parts(&ws, &src, dest.to_str().unwrap(), 2 * MIB).is_err());
        assert!(!dest.exists());
        assert!(leftovers(dest.parent().unwrap()).is_empty());
        assert!(leftovers(&local.0).is_empty());
    }
}
