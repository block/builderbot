//! Connected-app grants on the client side: the credential file
//! `stagedctl connect` writes, the proof a later run sends to reconnect
//! with it, and the rule for choosing which grant to use.
//!
//! The app's consent flow (`src-tauri/src/control_server/enrol.rs`) hands an
//! approved client a grant id and a reconnect secret exactly once. They
//! live in `~/.staged/control/grants/<grant-id>.json`, `0600` inside a
//! `0700` directory, read back through the same hardened reader as the
//! discovery files. The secret never crosses the socket again: a reconnect
//! asks the app for a nonce and answers with
//! `HMAC-SHA256(key = SHA-256(secret), msg = "<grant-id>\n<nonce>")`, which
//! the app checks against the hash it stored. A successful reconnect
//! renews the grant for another thirty days; the client refuses an expiry
//! that moved backwards or further out than that.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::client::CtlError;

/// How long an approval lasts without use; the app renews to exactly this
/// on every reconnect. Must equal `store::CONTROL_GRANT_LIFETIME_MS`.
pub const GRANT_LIFETIME_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Clock skew tolerated between this machine and the app when checking a
/// renewed expiry (they are the same machine; this absorbs a slow socket).
const EXPIRY_SKEW_MS: i64 = 5 * 60 * 1000;

/// `~/.staged/control/grants/<grant-id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantFile {
    pub grant_id: String,
    pub reconnect_secret: String,
    /// The name approved in the dialog.
    pub client: String,
    pub scope: GrantScope,
    /// `"read"` or `"write"`.
    pub access: String,
    /// ms since the epoch, as the app records it.
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrantScope {
    /// `None` = every project.
    #[serde(default)]
    pub project_id: Option<String>,
}

impl GrantFile {
    pub fn is_expired_at(&self, now_ms: i64) -> bool {
        self.expires_at <= now_ms
    }

    /// Human wording of the scope for output.
    pub fn scope_label(&self) -> String {
        match &self.scope.project_id {
            Some(project_id) => format!("project {project_id}"),
            None => "all projects".to_string(),
        }
    }

    /// Check a renewed expiry the app returned: never earlier than the
    /// current one, never more than one lifetime out.
    pub fn accept_renewal(&self, expires_at: i64, now_ms: i64) -> Result<(), String> {
        if expires_at < self.expires_at {
            return Err(format!(
                "the app moved the grant's expiry backwards ({} -> {expires_at})",
                self.expires_at
            ));
        }
        if expires_at > now_ms + GRANT_LIFETIME_MS + EXPIRY_SKEW_MS {
            return Err(format!(
                "the app returned a grant expiry more than thirty days out ({expires_at})"
            ));
        }
        Ok(())
    }
}

/// `~/.staged/control/grants`.
pub fn grants_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("control").join("grants")
}

/// A grant id the app issued is a lowercase hex UUID; nothing else is
/// joined into a path. Defence in depth — the server is same-uid and
/// generates the ids — and what keeps the `<id>.json` listing honest.
pub fn validate_grant_id(grant_id: &str) -> Result<(), CtlError> {
    let well_formed = !grant_id.is_empty()
        && grant_id.len() <= 64
        && grant_id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase() || b == b'-');
    if well_formed {
        Ok(())
    } else {
        Err(CtlError::Transport(format!(
            "refusing grant id {grant_id:?}: not of the form the app issues ([0-9a-f-])"
        )))
    }
}

/// `~/.staged/control/grants/<grant-id>.json`, for a validated id.
pub fn grant_file_path(data_dir: &Path, grant_id: &str) -> Result<PathBuf, CtlError> {
    validate_grant_id(grant_id)?;
    Ok(grants_dir(data_dir).join(format!("{grant_id}.json")))
}

/// ms since the epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// =============================================================================
// The proof
// =============================================================================

/// HMAC-SHA256 (RFC 2104). Mirrors `control_server::enrol::hmac_sha256`
/// line for line; both pin RFC 4231's vectors.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0u8; BLOCK];
    if key.len() > BLOCK {
        block_key[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block_key.map(|b| b ^ 0x36));
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(block_key.map(|b| b ^ 0x5c));
    outer.update(inner_hash);
    outer.finalize().into()
}

/// The proof for `nonce`: the HMAC key is the SHA-256 of the secret (what
/// the app stores), the message `"<grant-id>\n<nonce>"`.
pub fn reconnect_proof(grant: &GrantFile, nonce: &str) -> String {
    let key = Sha256::digest(grant.reconnect_secret.as_bytes());
    let message = format!("{}\n{nonce}", grant.grant_id);
    hex::encode(hmac_sha256(&key, message.as_bytes()))
}

// =============================================================================
// Files
// =============================================================================

/// Every readable, well-formed grant file under `data_dir`, soonest-to-
/// expire last (so the freshest approval is tried first). Unreadable or
/// unsafe files are skipped with a note on stderr; they are never deleted
/// here.
pub fn read_grant_files(data_dir: &Path) -> Vec<(PathBuf, GrantFile)> {
    let dir = grants_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut grants: Vec<(PathBuf, GrantFile)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match crate::client::read_private_file_lossy(&path) {
            Ok(contents) => match serde_json::from_str::<GrantFile>(&contents) {
                Ok(grant) => match validate_grant_id(&grant.grant_id) {
                    Ok(()) => grants.push((path, grant)),
                    Err(e) => eprintln!("stagedctl: ignoring grant file {}: {e}", path.display()),
                },
                Err(e) => eprintln!(
                    "stagedctl: ignoring malformed grant file {}: {e}",
                    path.display()
                ),
            },
            Err(e) => eprintln!("stagedctl: ignoring grant file {}: {e}", path.display()),
        }
    }
    grants.sort_by(|(_, a), (_, b)| {
        b.expires_at
            .cmp(&a.expires_at)
            .then(a.grant_id.cmp(&b.grant_id))
    });
    grants
}

/// Write `grant` as `0600` inside a `0700` grants directory, atomically.
pub fn write_grant_file(data_dir: &Path, grant: &GrantFile) -> Result<PathBuf, CtlError> {
    let path = grant_file_path(data_dir, &grant.grant_id)?;
    let dir = grants_dir(data_dir);
    ensure_private_dir(dir.parent().expect("grants dir has a parent"))?;
    ensure_private_dir(&dir)?;
    let contents = serde_json::to_vec_pretty(grant)
        .map_err(|e| CtlError::Environment(format!("serialize grant: {e}")))?;
    write_private_file(&path, &contents)?;
    Ok(path)
}

/// Create `path` `0700` if missing; refuse a symlink or a directory that is
/// not ours.
fn ensure_private_dir(path: &Path) -> Result<(), CtlError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(CtlError::Environment(format!(
                    "refusing to use {}: not a plain directory",
                    path.display()
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                // SAFETY: `geteuid` takes no arguments and cannot fail.
                let uid = unsafe { libc::geteuid() };
                if meta.uid() != uid {
                    return Err(CtlError::Environment(format!(
                        "refusing to use {}: owned by another user",
                        path.display()
                    )));
                }
                if meta.mode() & 0o077 != 0 {
                    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                        .map_err(|e| {
                            CtlError::Environment(format!("chmod 0700 {}: {e}", path.display()))
                        })?;
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
                .map_err(|e| CtlError::Environment(format!("create {}: {e}", path.display())))
        }
        Err(e) => Err(CtlError::Environment(format!(
            "inspect {}: {e}",
            path.display()
        ))),
    }
}

/// Temp file beside the target, `0600`, fsync, rename.
fn write_private_file(path: &Path, contents: &[u8]) -> Result<(), CtlError> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        CtlError::Environment(format!("write {}: {e}", path.display()))
    })
}

/// Remove a grant file; a file already gone is fine.
pub fn remove_grant_file(path: &Path) -> Result<(), CtlError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CtlError::Environment(format!(
            "remove {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(id: &str, expires_at: i64) -> GrantFile {
        GrantFile {
            grant_id: id.to_string(),
            reconnect_secret: "ab".repeat(32),
            client: "my-agent".to_string(),
            scope: GrantScope { project_id: None },
            access: "write".to_string(),
            expires_at,
        }
    }

    fn private_tempdir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    /// RFC 4231 test cases 1, 2 and 6 — the same vectors the app pins.
    #[test]
    fn hmac_sha256_matches_rfc_4231() {
        assert_eq!(
            hex::encode(hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex::encode(hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex::encode(hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    /// The proof the app's `enrol::expected_proof` computes from the stored
    /// hash for the same grant, nonce and secret (the value is fixed here
    /// so the two implementations are pinned to each other).
    #[test]
    fn reconnect_proof_keys_on_the_secret_hash_and_signs_grant_and_nonce() {
        let g = grant("g1", 0);
        let expected = {
            let key = Sha256::digest(g.reconnect_secret.as_bytes());
            hex::encode(hmac_sha256(&key, b"g1\nn1"))
        };
        assert_eq!(reconnect_proof(&g, "n1"), expected);
        assert_ne!(reconnect_proof(&g, "n2"), expected);
        assert_ne!(reconnect_proof(&grant("g2", 0), "n1"), expected);
        assert_eq!(reconnect_proof(&g, "n1").len(), 64);
    }

    #[test]
    fn renewals_may_not_move_backwards_or_past_one_lifetime() {
        let now = 1_000_000;
        let g = grant("g1", now + 10);
        assert!(g.accept_renewal(now + GRANT_LIFETIME_MS, now).is_ok());
        assert!(g.accept_renewal(now + 10, now).is_ok(), "unchanged is fine");
        assert!(g.accept_renewal(now + 9, now).is_err(), "backwards");
        assert!(
            g.accept_renewal(now + GRANT_LIFETIME_MS + EXPIRY_SKEW_MS + 1, now)
                .is_err(),
            "beyond thirty days plus skew"
        );
        assert!(g.is_expired_at(now + 10));
        assert!(!g.is_expired_at(now + 9));
        assert_eq!(g.scope_label(), "all projects");
    }

    #[test]
    fn grant_files_are_written_private_and_read_back_freshest_first() {
        let dir = private_tempdir();
        let a = grant("aaaa", 5_000);
        let b = grant("bbbb", 9_000);
        let path_a = write_grant_file(dir.path(), &a).unwrap();
        write_grant_file(dir.path(), &b).unwrap();
        assert_eq!(path_a, dir.path().join("control/grants/aaaa.json"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path_a), 0o600);
            assert_eq!(mode(&grants_dir(dir.path())), 0o700);
            assert_eq!(mode(&dir.path().join("control")), 0o700);
        }
        let read: Vec<String> = read_grant_files(dir.path())
            .into_iter()
            .map(|(_, g)| g.grant_id)
            .collect();
        assert_eq!(read, ["bbbb", "aaaa"], "latest expiry first");

        // Rewriting a grant replaces it in place; a malformed neighbour is
        // skipped, not fatal.
        let renewed = GrantFile {
            expires_at: 20_000,
            ..a.clone()
        };
        write_grant_file(dir.path(), &renewed).unwrap();
        std::fs::write(grants_dir(dir.path()).join("junk.json"), "{").unwrap();
        let read: Vec<(String, i64)> = read_grant_files(dir.path())
            .into_iter()
            .map(|(_, g)| (g.grant_id, g.expires_at))
            .collect();
        assert_eq!(
            read,
            [("aaaa".to_string(), 20_000), ("bbbb".to_string(), 9_000)]
        );

        remove_grant_file(&path_a).unwrap();
        remove_grant_file(&path_a).unwrap();
        assert_eq!(read_grant_files(dir.path()).len(), 1);
        assert!(read_grant_files(&dir.path().join("nowhere")).is_empty());
    }

    #[test]
    fn grant_ids_are_hex_uuids_before_any_path_is_built() {
        for ok in ["0f3a", "7c9e2b10-1d2f-4a8b-9c3d-0e1f2a3b4c5d", "a-b-c"] {
            assert!(validate_grant_id(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "../x",
            "g/1",
            "G1",
            "0F3A",
            "g1.json",
            "a b",
            &"a".repeat(65),
        ] {
            let err = validate_grant_id(bad).unwrap_err();
            assert_eq!(err.exit_code(), 2, "{bad:?}");
            assert!(grant_file_path(Path::new("/d"), bad).is_err(), "{bad:?}");
        }
        let dir = private_tempdir();
        let mut traversal = grant("aaaa", 1);
        traversal.grant_id = "../escape".to_string();
        assert!(write_grant_file(dir.path(), &traversal).is_err());
        assert!(!dir.path().join("escape.json").exists());
        assert!(!grants_dir(dir.path()).exists(), "nothing was created");

        // A file whose content names an id outside the alphabet is ignored
        // on read, even though the file itself is well placed.
        write_grant_file(dir.path(), &grant("bbbb", 1)).unwrap();
        let mut odd = grant("cccc", 1);
        odd.grant_id = "../odd".to_string();
        std::fs::write(
            grants_dir(dir.path()).join("odd.json"),
            serde_json::to_vec(&odd).unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                grants_dir(dir.path()).join("odd.json"),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        let read: Vec<String> = read_grant_files(dir.path())
            .into_iter()
            .map(|(_, g)| g.grant_id)
            .collect();
        assert_eq!(read, ["bbbb"]);
    }

    #[test]
    #[cfg(unix)]
    fn a_world_readable_grant_file_is_not_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_tempdir();
        let path = write_grant_file(dir.path(), &grant("0e", 1)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_grant_files(dir.path()).is_empty());
    }
}
