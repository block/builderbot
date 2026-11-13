//! The private npm download cache (`npm-prefix/cache`) and its size cap.
//!
//! Every npm invocation Staged spawns — bridge installs, Doctor's
//! `npm install -g` fixes, the registry probes — shares one cacache under
//! `<packages>/npm-prefix/cache/_cacache`. npm never evicts from it: each
//! floating install adds the new version's tarballs beside every older one,
//! and each revalidated package document leaves its previous body behind
//! unreferenced. The cache has no correctness role — a bridge at runtime
//! never reads it, and an install against an empty cache only re-downloads —
//! so the reconcile epilogue wipes it whole once it outgrows
//! [`NPM_CACHE_MAX_BYTES`] (see [`prune_npm_cache_at`]).

use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "tests/cache.rs"]
mod tests;

/// Size cap for the private npm cache. A cache refilled by one reconcile
/// pass is roughly 0.4 GB, so this leaves room for a couple of weeks of
/// bridge releases between wipes.
pub(super) const NPM_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// npm's content-addressed store inside the cache dir — the only part that
/// grows without bound (`_logs` beside it is capped by npm itself).
const CACACHE_DIRNAME: &str = "_cacache";

/// Prefix of the sibling an oversized `_cacache` is renamed to before it is
/// deleted (`_cacache.trash-<pid>`).
const CACACHE_TRASH_PREFIX: &str = "_cacache.trash-";

/// `<npm-prefix>/cache` — the one definition of where the private npm cache
/// lives, shared by the env pairs Doctor's fixes run under and the bridge
/// installer.
pub(super) fn npm_cache_dir_in_prefix(npm_prefix: &Path) -> PathBuf {
    npm_prefix.join("cache")
}

/// `<packages>/npm-prefix/cache` — [`npm_cache_dir_in_prefix`] for a
/// packages root.
pub(super) fn npm_cache_dir(packages_root: &Path) -> PathBuf {
    npm_cache_dir_in_prefix(&packages_root.join("npm-prefix"))
}

/// Wipe the private npm cache's `_cacache` when it holds more than
/// `max_bytes`. Best-effort: returns nothing and logs every failure at warn
/// level, since a cache that could not be pruned costs disk, never
/// correctness, and is retried on the next reconcile pass.
///
/// Three moves:
///
/// 1. Remove any `_cacache.trash-*` sibling an interrupted prune left behind.
/// 2. Sum the file sizes under `_cacache` without following symlinks, and
///    stop when the total is within the cap.
/// 3. Rename `_cacache` to `_cacache.trash-<pid>` and delete that. The rename
///    is atomic, so npm sees either the whole cache or none of it — never a
///    half-deleted one whose index points at missing content.
///
/// The caller holds the tool-install mutex and the packages flock, so no
/// Staged-spawned npm install is writing the cache meanwhile. Blocking
/// filesystem work: call it from `spawn_blocking`.
pub(super) fn prune_npm_cache_at(packages_root: &Path, max_bytes: u64) {
    let cache_dir = npm_cache_dir(packages_root);
    remove_leftover_trash(&cache_dir);

    let cacache = cache_dir.join(CACACHE_DIRNAME);
    match std::fs::symlink_metadata(&cacache) {
        Ok(meta) if meta.is_dir() => {}
        // A plain file or a symlink where npm's store should be is not ours
        // to size or delete; npm reports it on the next install.
        Ok(_) => return,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            log::warn!(
                "skipping npm cache prune: cannot inspect {}: {error}",
                cacache.display()
            );
            return;
        }
    }

    let size = dir_size(&cacache);
    if size <= max_bytes {
        return;
    }

    let trash = cache_dir.join(format!("{CACACHE_TRASH_PREFIX}{}", std::process::id()));
    if let Err(error) = std::fs::rename(&cacache, &trash) {
        log::warn!(
            "failed to prune the npm cache ({size} bytes, cap {max_bytes}): move {} aside: {error}",
            cacache.display()
        );
        return;
    }
    match std::fs::remove_dir_all(&trash) {
        Ok(()) => log::info!(
            "pruned the npm cache at {}: reclaimed {size} bytes (cap {max_bytes} bytes)",
            cache_dir.display()
        ),
        // The cache itself is already gone from npm's point of view; what is
        // left of the trash dir is swept by the next prune.
        Err(error) => log::warn!(
            "npm cache ({size} bytes, cap {max_bytes}) was moved aside but {} could not be fully deleted: {error}",
            trash.display()
        ),
    }
}

/// Remove every `_cacache.trash-*` entry in the cache dir — what a prune that
/// was interrupted (crash, quit, power loss) between its rename and the end
/// of its delete left behind.
fn remove_leftover_trash(cache_dir: &Path) {
    let entries = match std::fs::read_dir(cache_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            log::warn!(
                "cannot scan {} for interrupted npm cache prunes: {error}",
                cache_dir.display()
            );
            return;
        }
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(CACACHE_TRASH_PREFIX)
        {
            continue;
        }
        let path = entry.path();
        // `file_type` does not follow symlinks, so a symlink named like a
        // trash dir is unlinked rather than traversed.
        let result = match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            log::warn!(
                "failed to remove interrupted npm cache prune leftover {}: {error}",
                path.display()
            );
        }
    }
}

/// Total size in bytes of the regular files under `dir`. Symlinks are
/// counted as nothing and never followed, so a link out of the cache can
/// neither inflate the total nor send the walk elsewhere. An unreadable
/// entry is logged and skipped — the total is then a lower bound, which can
/// only delay a prune.
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(error) => {
                log::warn!(
                    "npm cache sizing skipped unreadable {}: {error}",
                    current.display()
                );
                continue;
            }
        };
        for entry in entries.flatten() {
            // `DirEntry::metadata` does not traverse symlinks.
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(error) => {
                    log::warn!(
                        "npm cache sizing skipped unreadable {}: {error}",
                        entry.path().display()
                    );
                    continue;
                }
            };
            if meta.is_dir() {
                pending.push(entry.path());
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}
