//! Git LFS objects, carried beside the bundle.
//!
//! A bundle holds LFS pointers, not the content they name, and a Blox clone
//! has no LFS filter configured, so without this the destination would check
//! pointers out. [`SNAPSHOT_SCRIPT`](super::SNAPSHOT_SCRIPT) lists the LFS
//! objects the snapshot refers to — in the `W` and `I` trees, and in the
//! commits the bundle carries that no remote has — and packs the ones the
//! source has into `<bundle>.lfs.tar`, which travels like the bundle. The
//! archive keeps the object store's `objects/xx/yy/<oid>` layout, plus a
//! manifest, `oids`: a line `<oid> <size> <in W> <at the source>` per object,
//! the flags `1` or `0`.
//!
//! [`APPLY_SCRIPT`](super::APPLY_SCRIPT) unpacks the archive into the
//! destination's object store, fails if an object `W` uses and the source had
//! did not arrive, sets the LFS filter up unless some config scope already
//! has it, fetches from origin what the source lacked (best-effort), and once
//! the working tree is restored runs `git lfs checkout`, which replaces the
//! pointers it can with content. Every checkout and `read-tree -u` runs with
//! `GIT_LFS_SKIP_SMUDGE=1` (see `prelude!`): a smudge filter that can't find
//! an object fails the whole checkout. Verification compares tree hashes,
//! which pointers and content clean to alike.

use super::{ScriptReport, TransferError};

/// Definitions for the scripts that read or write LFS objects, after
/// `prelude!`.
macro_rules! lfs_prelude {
    () => {
        concat!(
            // Whether the repository uses LFS: an attributes file in one of
            // the given trees routes paths through the filter, or this
            // repository configures it. A filter configured globally doesn't
            // count; `git lfs install` puts it there for every repository.
            "uses_lfs() {\n",
            "  for _t; do G grep -q -e 'filter=lfs' \"$_t\" -- '*.gitattributes' 2>/dev/null && return 0; done\n",
            "  grep -qs -e 'filter=lfs' \"$(G rev-parse --git-path info/attributes)\" && return 0\n",
            "  G config --local --get-regexp '^filter\\.lfs\\.' >/dev/null 2>&1\n",
            "}\n",
            // The object store, shared by every worktree: `lfs.storage`
            // (relative to the common git directory), else `lfs` there.
            "lfs_dir() {\n",
            "  _c=$(G rev-parse --git-common-dir) && _c=$(cd -- \"$_c\" && pwd -P) || return 1\n",
            "  _s=$(G config --get lfs.storage) || _s=lfs\n",
            "  case $_s in /*) printf '%s\\n' \"$_s\" ;; *) printf '%s/%s\\n' \"$_c\" \"$_s\" ;; esac\n",
            "}\n",
            // stdin: `<blob> w|o` lines (`w`: in the working tree snapshot).
            // stdout: `<oid> <size> <1 if in W, else 0>` per LFS object those
            // blobs point at, once each. Pointers are small text blobs (git
            // lfs looks no further than 1024 bytes); a header carries its
            // blob's tag, so `staged-lfs-` keeps a binary blob's line from
            // passing for one.
            "lfs_pointers() {\n",
            "  LC_ALL=C sort -u | awk '$1 != o { if (o != \"\") print o, \"staged-lfs-\" t; o = $1; t = $2; next } $2 == \"w\" { t = \"w\" } END { if (o != \"\") print o, \"staged-lfs-\" t }' \\\n",
            "  | G cat-file --batch-check='%(objecttype) %(objectsize) %(objectname) %(rest)' \\\n",
            "  | awk '$1 == \"blob\" && $2 < 1024 { print $3, $4 }' \\\n",
            "  | G cat-file --batch='%(objectname) %(objecttype) %(objectsize) %(rest)' \\\n",
            "  | awk '\n",
            "    function flush() { if (v && length(lo) == 64 && lo !~ /[^0-9a-f]/ && ls ~ /^[0-9]+$/) print lo, ls, w; v = 0 }\n",
            "    /^[0-9a-f]+ blob [0-9]+ staged-lfs-[wo]$/ { flush(); w = ($4 == \"staged-lfs-w\") ? 1 : 0; n = 0; lo = \"\"; ls = \"\"; next }\n",
            "    { n++ }\n",
            "    n == 1 { v = ($0 == \"version https://git-lfs.github.com/spec/v1\" || $0 == \"version https://hawser.github.com/spec/v1\"); next }\n",
            "    $1 == \"oid\" && substr($2, 1, 7) == \"sha256:\" { lo = substr($2, 8); next }\n",
            "    $1 == \"size\" { ls = $2 }\n",
            "    END { flush() }' \\\n",
            "  | LC_ALL=C sort | awk '$1 != o { if (o != \"\") print o, s, w; o = $1; s = $2; w = $3; next } $3 == 1 { w = 1 } END { if (o != \"\") print o, s, w }'\n",
            "}\n",
            // stdin: lines starting `<oid> <size>`; stdout: each with ` 1`
            // appended when the store at `$1` has the object at that size,
            // else ` 0`.
            "lfs_have() {\n",
            "  _m=$(cat)\n",
            "  [ -n \"$_m\" ] || return 0\n",
            "  { printf '%s\\n' \"$_m\" | awk '{ print \"objects/\" substr($1, 1, 2) \"/\" substr($1, 3, 2) \"/\" $1 }' | (cd -- \"$1\" 2>/dev/null && xargs ls -lnL -- 2>/dev/null)\n",
            "    printf 'staged-lfs-manifest\\n%s\\n' \"$_m\"\n",
            "  } | awk '\n",
            "    m { k = \"objects/\" substr($1, 1, 2) \"/\" substr($1, 3, 2) \"/\" $1; p = 0; if ((k in have) && have[k] == $2 + 0) p = 1; print $0, p; next }\n",
            "    $0 == \"staged-lfs-manifest\" { m = 1; next }\n",
            "    { have[$NF] = $5 + 0 }'\n",
            "}\n",
            // stdin: `lfs_have` output; stdout: the report's counts. Sums go
            // through `%.0f`, as mawk prints a large integer in exponent form.
            "lfs_counts() {\n",
            "  awk '{ if ($NF == 1) { p++; pb += $2 } else { m++; mb += $2 } } END { printf \"lfs_total=%d\\nlfs_present=%d\\nlfs_present_bytes=%.0f\\nlfs_missing=%d\\nlfs_missing_bytes=%.0f\\n\", p + m, p, pb, m, mb }'\n",
            "}\n",
        )
    };
}

/// How many LFS objects a snapshot refers to, and how many the source has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LfsCounts {
    pub total: u64,
    pub present: u64,
    pub present_bytes: u64,
    /// Objects the source never downloaded: pushed content, which the
    /// destination fetches from origin if it can.
    pub missing: u64,
    pub missing_bytes: u64,
}

impl LfsCounts {
    /// The `lfs_*` counts from a report, if it has them.
    pub fn from_report(report: &ScriptReport) -> Option<Self> {
        let count = |key: &str| report.get(key).and_then(|v| v.parse::<u64>().ok());
        Some(Self {
            total: count("lfs_total")?,
            present: count("lfs_present")?,
            present_bytes: count("lfs_present_bytes")?,
            missing: count("lfs_missing")?,
            missing_bytes: count("lfs_missing_bytes")?,
        })
    }
}

/// The LFS objects a snapshot carries: what [`SNAPSHOT_SCRIPT`]
/// (super::SNAPSHOT_SCRIPT) reports when it wrote an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LfsPayload {
    pub counts: LfsCounts,
    /// `<bundle>.lfs.tar`, beside the bundle at each end.
    pub archive: super::BundleFile,
}

impl LfsPayload {
    /// The archive's file name for a bundle's.
    pub fn archive_name(bundle_name: &str) -> String {
        format!("{bundle_name}.lfs.tar")
    }

    pub(super) fn parse(
        step: &'static str,
        report: &ScriptReport,
    ) -> Result<Option<Self>, TransferError> {
        if report.get("lfs").is_none() {
            return Ok(None);
        }
        let counts = LfsCounts::from_report(report).ok_or_else(|| TransferError::Protocol {
            step,
            message: "missing LFS counts".to_string(),
        })?;
        Ok(Some(Self {
            counts,
            archive: super::BundleFile::from_report_as(step, report, "lfs")?,
        }))
    }
}
