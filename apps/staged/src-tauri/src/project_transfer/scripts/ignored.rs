//! Ignored files: backed up before the source comes down, and carried to the
//! destination when asked.
//!
//! Which ones: the regular files `git status --ignored=matching` lists one by
//! one — an ignored file outside any directory an ignore pattern matches,
//! including each file of an untracked directory holding nothing but ignored
//! files — so `.env` and `debug.log` are in, and `node_modules/`, `target/`
//! and `.venv/` are not: they are reported as a directory, never walked, and
//! can be rebuilt. Symlinks are left out, and so is a name with a newline in
//! it, which lets every list past the selection be one path per line. The
//! preflight counts the same set ([`PREFLIGHT_SCRIPT`](super::PREFLIGHT_SCRIPT)'s
//! `ignored_files`), so what the dialog says is what moves.
//!
//! [`IGNORED_ARCHIVE_SCRIPT`] packs them into `<bundle stem>.ignored.tar`, a
//! plain archive of `./<path>` entries — `tar -xf` it in a checkout to put
//! them back — unless they come to more than the cap, when it says so and
//! writes nothing. [`APPLY_SCRIPT`](super::APPLY_SCRIPT) unpacks one into a
//! staging directory and places each file into the checkout itself, once the
//! working tree is in place: only a path the destination ignores too (so
//! never a tracked one, and never one that would show up as a change of the
//! branch's own); never over anything already there; and never through a
//! symlink, or where a file stands in for a directory, so nothing lands
//! outside the checkout. Each placed file is compared with what was archived.
//! Tree hashes are untouched: ignored files are in neither snapshot.

use super::{BundleFile, ScriptReport, TransferError};

/// The most a transfer archives of a repository's ignored files. Past it
/// they are neither backed up nor carried.
pub const IGNORED_FILES_CAP: u64 = 256 * 1024 * 1024;

/// Definitions for the scripts that list, pack or place ignored files, after
/// `prelude!`.
macro_rules! ignored_prelude {
    () => {
        concat!(
            // One `./<path>` line per ignored file to carry, for the checkout
            // at `.`. `-z` gives names as they are; `--no-renames` keeps
            // every record one path. A record ending in `/` is a directory an
            // ignore pattern matches, not walked.
            "ignored_list() {\n",
            "  G --no-optional-locks status --porcelain=v1 -z --no-renames --untracked-files=all --ignored=matching 2>/dev/null \\\n",
            "  | xargs -0 sh -c 'nl=\"\n\"; for e; do case $e in *\"$nl\"*|\"!! \"*/) ;; \"!! \"*) p=${e#\"!! \"}; [ -f \"$p\" ] && [ ! -L \"$p\" ] && printf \"./%s\\n\" \"$p\" ;; esac; done' sh\n",
            "}\n",
            // stdin: path lines; stdout: `<count> <bytes>` of the regular
            // files among them. With no input, GNU xargs runs `ls` once
            // anyway, on `.`, which the `-` filter drops. `%.0f`, as mawk
            // prints a large integer in exponent form.
            "ignored_sizes() {\n",
            "  tr '\\n' '\\000' | xargs -0 ls -lndq -- 2>/dev/null | awk '$1 ~ /^-/ { n++; b += $5 } END { printf \"%d %.0f\\n\", n, b }'\n",
            "}\n",
            // Lines in a file, for a report.
            "ignored_count() { wc -l < \"$1\" | tr -d ' \\t'; }\n",
            // Place the archive `$ignored` (checked against
            // `$ignored_sha256`) into the checkout at `.`; see the module
            // documentation. Prints `ignored_placed`, and for what it left
            // out `ignored_unignored` / `ignored_in_the_way` with one path
            // of each as an example. Uses `fail` for anything it can't do.
            "ignored_place() {\n",
            "  [ \"$(sha256_of \"$ignored\")\" = \"$ignored_sha256\" ] || fail 'the ignored files do not match their checksum'\n",
            "  _st=\"$ignored.d\"\n",
            "  rm -rf -- \"$_st\"\n",
            "  mkdir -p -- \"$_st/files\" || fail \"cannot create $_st\"\n",
            "  _err=$(tar -xf \"$ignored\" -C \"$_st/files\" 2>&1) || fail \"cannot unpack the ignored files: $_err\"\n",
            "  (cd -- \"$_st/files\" && find . -type f) | LC_ALL=C sort > \"$_st/all\" || fail 'cannot list the ignored files'\n",
            "  : > \"$_st/free\"\n",
            "  : > \"$_st/in_the_way\"\n",
            // First what is in the way: anything at the path, and a symlink
            // or a file where a directory on the way goes. Those go before
            // check-ignore, which stops at a path beyond a symlink.
            "  while IFS= read -r _p; do\n",
            "    _r=${_p#./}\n",
            "    _ok=1\n",
            "    case /$_r/ in */../*|*/./*|*//*|*/.git/*) _ok=0 ;; esac\n",
            "    _d=$_r\n",
            "    while [ \"$_ok\" = 1 ]; do\n",
            "      case $_d in */*) _d=${_d%/*} ;; *) break ;; esac\n",
            "      if [ -L \"./$_d\" ] || { [ -e \"./$_d\" ] && [ ! -d \"./$_d\" ]; }; then _ok=0; fi\n",
            "    done\n",
            "    if [ \"$_ok\" = 1 ] && { [ -e \"./$_r\" ] || [ -L \"./$_r\" ]; }; then _ok=0; fi\n",
            "    if [ \"$_ok\" = 1 ]; then printf '%s\\n' \"$_p\" >> \"$_st/free\"; else printf '%s\\n' \"$_r\" >> \"$_st/in_the_way\"; fi\n",
            "  done < \"$_st/all\"\n",
            // Then what this checkout ignores too: check-ignore leaves out
            // tracked paths and the rest. It exits 1 when it ignores none; if
            // it can't answer at all, nothing is placed.
            "  tr '\\n' '\\000' < \"$_st/free\" | G check-ignore -z --stdin > \"$_st/ignored0\" 2>/dev/null\n",
            "  [ $? -le 1 ] || : > \"$_st/ignored0\"\n",
            "  tr '\\000' '\\n' < \"$_st/ignored0\" | LC_ALL=C sort > \"$_st/ignored\"\n",
            "  LC_ALL=C comm -23 \"$_st/free\" \"$_st/ignored\" > \"$_st/unignored\"\n",
            "  : > \"$_st/placed\"\n",
            "  while IFS= read -r _p; do\n",
            "    _r=${_p#./}\n",
            "    case $_r in */*) [ -d \"./${_r%/*}\" ] || mkdir -p -- \"./${_r%/*}\" || fail \"cannot create the directory for $_r\" ;; esac\n",
            "    cp -p \"$_st/files/$_r\" \"./$_r\" || fail \"cannot place $_r\"\n",
            "    cmp -s \"$_st/files/$_r\" \"./$_r\" || fail \"$_r did not arrive whole\"\n",
            "    printf '%s\\n' \"$_r\" >> \"$_st/placed\"\n",
            "  done < \"$_st/ignored\"\n",
            "  printf 'ignored_placed=%s\\n' \"$(ignored_count \"$_st/placed\")\"\n",
            "  if [ -s \"$_st/unignored\" ]; then\n",
            "    printf 'ignored_unignored=%s\\nignored_unignored_path=%s\\n' \"$(ignored_count \"$_st/unignored\")\" \"$(head -n 1 \"$_st/unignored\" | sed 's|^\\./||')\"\n",
            "  fi\n",
            "  if [ -s \"$_st/in_the_way\" ]; then\n",
            "    printf 'ignored_in_the_way=%s\\nignored_in_the_way_path=%s\\n' \"$(ignored_count \"$_st/in_the_way\")\" \"$(head -n 1 \"$_st/in_the_way\")\"\n",
            "  fi\n",
            "  rm -rf -- \"$_st\"\n",
            "}\n",
        )
    };
}

/// Archive a checkout's ignored files (see the module documentation).
///
/// Arguments: `$1` the checkout, `$2` the archive to write (absolute; its
/// directory is created), `$3` the cap in bytes.
///
/// Prints `ignored_files` and `ignored_bytes`. With none, that is all. Past
/// the cap it adds `ignored_over_cap=1` and writes nothing. Otherwise it
/// replaces `$2` whole — through `$2.part`, so an archive already there
/// stays until this one is complete — and prints its `ignored_size` and
/// `ignored_sha256`. Reads the checkout only.
pub const IGNORED_ARCHIVE_SCRIPT: &str = concat!(
    prelude!(),
    ignored_prelude!(),
    "cd -- \"$1\" 2>/dev/null || fail \"cannot enter $1\"\n",
    "ar=$2 cap=$3\n",
    "case $cap in ''|*[!0-9]*) fail \"not a size: $cap\" ;; esac\n",
    "top=$(G rev-parse --show-toplevel 2>/dev/null) || fail \"not a git checkout: $PWD\"\n",
    "cd -- \"$top\" || fail \"cannot enter $top\"\n",
    "mkdir -p -- \"$(dirname -- \"$ar\")\" || fail \"cannot create the directory for $ar\"\n",
    "list=\"$ar.list\"\n",
    "on_fail() { rm -f -- \"$list\" \"$ar.part\"; }\n",
    "ignored_list > \"$list\" || fail 'cannot list the ignored files'\n",
    "sizes=$(ignored_sizes < \"$list\")\n",
    "n=${sizes% *} b=${sizes#* }\n",
    "printf 'ignored_files=%s\\nignored_bytes=%s\\n' \"$n\" \"$b\"\n",
    "if [ \"$n\" = 0 ]; then rm -f -- \"$list\"; exit 0; fi\n",
    "if [ \"$b\" -gt \"$cap\" ]; then rm -f -- \"$list\"; printf 'ignored_over_cap=1\\n'; exit 0; fi\n",
    // bsdtar (macOS) would add AppleDouble `._` entries and extended
    // attributes, as for the LFS archive. GNU tar exits 1 when a file changed
    // while it was read (a log being written): what it read is the backup.
    "tar_c=tar gnu=1\n",
    "if tar --version 2>/dev/null | grep -q bsdtar; then tar_c='tar --no-mac-metadata --no-xattrs' gnu=0; export COPYFILE_DISABLE=1; fi\n",
    "rm -f -- \"$ar.part\"\n",
    "err=$(tr '\\n' '\\000' < \"$list\" | $tar_c -cf \"$ar.part\" --null -T - 2>&1)\n",
    "rc=$?\n",
    "if [ \"$rc\" != 0 ] && ! { [ \"$gnu\" = 1 ] && [ \"$rc\" = 1 ] && [ -s \"$ar.part\" ]; }; then fail \"cannot archive the ignored files: $err\"; fi\n",
    "mv -f -- \"$ar.part\" \"$ar\" || fail \"cannot write $ar\"\n",
    "rm -f -- \"$list\"\n",
    "printf 'ignored_size=%s\\nignored_sha256=%s\\n' \"$(size_of \"$ar\")\" \"$(sha256_of \"$ar\")\"\n",
    "exit 0\n",
);

/// What [`IGNORED_ARCHIVE_SCRIPT`] found and wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredFiles {
    pub files: u64,
    pub bytes: u64,
    /// More than the cap: nothing was archived.
    pub over_cap: bool,
    /// The archive, when there was anything to archive within the cap.
    pub archive: Option<BundleFile>,
}

impl IgnoredFiles {
    /// The archive's file name for a bundle's: `0-widgets.bundle` keeps its
    /// ignored files in `0-widgets.ignored.tar`.
    pub fn archive_name(bundle_name: &str) -> String {
        let stem = bundle_name.strip_suffix(".bundle").unwrap_or(bundle_name);
        format!("{stem}.ignored.tar")
    }

    pub fn parse(stdout: &str) -> Result<Self, TransferError> {
        const STEP: &str = "ignored files";
        let report = ScriptReport::parse(stdout);
        let files = report.require_u64(STEP, "ignored_files")?;
        let bytes = report.require_u64(STEP, "ignored_bytes")?;
        let over_cap = report.get("ignored_over_cap") == Some("1");
        let archive = if files > 0 && !over_cap {
            Some(BundleFile::from_report_as(STEP, &report, "ignored")?)
        } else {
            None
        };
        Ok(Self {
            files,
            bytes,
            over_cap,
            archive,
        })
    }
}

/// Arguments for [`IGNORED_ARCHIVE_SCRIPT`].
pub fn archive_args(checkout: &str, archive: &str, cap: u64) -> Vec<String> {
    vec![checkout.to_string(), archive.to_string(), cap.to_string()]
}

/// What placing an archive at the destination did: part of
/// [`ApplyReport`](super::ApplyReport).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoredPlacement {
    pub placed: u64,
    /// Files the destination doesn't ignore (its `.gitignore` or excludes
    /// differ), so they were left out rather than pass for the branch's own
    /// changes there.
    pub unignored: u64,
    pub unignored_example: Option<String>,
    /// Files with something already in the way: a file at the path, or a
    /// symlink or file where one of its directories goes.
    pub in_the_way: u64,
    pub in_the_way_example: Option<String>,
}

impl IgnoredPlacement {
    /// The placement a report describes, if it placed an archive.
    pub(super) fn from_report(report: &ScriptReport) -> Option<Self> {
        let count = |key: &str| report.get(key).and_then(|v| v.parse().ok()).unwrap_or(0);
        let example = |key: &str| {
            report
                .get(key)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        report.get("ignored_placed")?;
        Some(Self {
            placed: count("ignored_placed"),
            unignored: count("ignored_unignored"),
            unignored_example: example("ignored_unignored_path"),
            in_the_way: count("ignored_in_the_way"),
            in_the_way_example: example("ignored_in_the_way_path"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_the_archive_script_reports() {
        let sha = "AB".repeat(32);
        let written = IgnoredFiles::parse(&format!(
            "ignored_files=3\nignored_bytes=120\nignored_size=10240\nignored_sha256={sha}\n"
        ))
        .unwrap();
        assert_eq!(
            written,
            IgnoredFiles {
                files: 3,
                bytes: 120,
                over_cap: false,
                archive: Some(BundleFile {
                    size: 10240,
                    sha256: "ab".repeat(32)
                }),
            }
        );
        let none = IgnoredFiles::parse("ignored_files=0\nignored_bytes=0\n").unwrap();
        assert_eq!(none.archive, None);
        let over = IgnoredFiles::parse(
            "ignored_files=2\nignored_bytes=999999999999\nignored_over_cap=1\n",
        )
        .unwrap();
        assert!(over.over_cap);
        assert_eq!(over.archive, None);
        // Files within the cap promise an archive.
        assert!(IgnoredFiles::parse("ignored_files=1\nignored_bytes=1\n").is_err());
        assert!(IgnoredFiles::parse("").is_err());
        assert_eq!(
            IgnoredFiles::archive_name("0-widgets.bundle"),
            "0-widgets.ignored.tar"
        );
    }

    #[test]
    fn a_placement_report_names_what_was_left_out() {
        let report = ScriptReport::parse(
            "ignored_placed=2\nignored_unignored=1\nignored_unignored_path=.envrc\n\
             ignored_in_the_way=3\nignored_in_the_way_path=cache/x.log\n",
        );
        assert_eq!(
            IgnoredPlacement::from_report(&report),
            Some(IgnoredPlacement {
                placed: 2,
                unignored: 1,
                unignored_example: Some(".envrc".to_string()),
                in_the_way: 3,
                in_the_way_example: Some("cache/x.log".to_string()),
            })
        );
        assert_eq!(
            IgnoredPlacement::from_report(&ScriptReport::parse("applied=x\n")),
            None
        );
    }
}
