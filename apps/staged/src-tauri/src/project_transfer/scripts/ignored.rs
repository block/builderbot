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
//! writes nothing. Beside it, `<bundle stem>.ignored.excludes` lists the ones
//! only this machine's own rules ignore — the clone's `info/exclude`, the
//! global `core.excludesFile` — rather than a `.gitignore` in the checkout,
//! which a destination checks out too. [`APPLY_SCRIPT`](super::APPLY_SCRIPT)
//! unpacks the archive into a staging directory and places each file into the
//! checkout itself, once the working tree is in place: only a path the
//! destination ignores too (so never a tracked one, and never one that would
//! show up as a change of the branch's own); never over anything already
//! there; and never through a symlink, or where a file stands in for a
//! directory, so nothing lands outside the checkout. Each placed file is
//! compared with what was archived. Tree hashes are untouched: ignored files
//! are in neither snapshot.
//!
//! Two things make the destination ignore what the source did. A path on the
//! excludes list that the destination doesn't ignore gets an anchored entry
//! (`/<path>`) in its `info/exclude` first, as its rules can't be expected
//! there: a fresh workspace clone has none. That file is the repository's,
//! read by each of its worktrees, so a path another worktree has anything at
//! gets no entry and is left out; and each entry is noted, before it is
//! added, where the caller says, so a rollback can take exactly those back
//! out (see [`super::super::undo::AddedExcludes`]). And an archived
//! `.gitignore` — a cache directory's own `*` (`.pytest_cache/`,
//! `.mypy_cache/`, a uv `.venv/`), which ignores itself and everything beside
//! it — is put in place before asking, as until it is there nothing it
//! ignores is; any that turns out not to be ignored itself is taken back out,
//! and the question asked again without it.

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
            // ignore pattern matches, not walked. Each batch exits 0 whatever
            // its last record was; a status that fails adds a record no
            // status prints (after a NUL, which ends any record it cut
            // short), and the batch that sees it exits 1, so the listing
            // fails rather than come out short.
            "ignored_list() {\n",
            "  { G --no-optional-locks status --porcelain=v1 -z --no-renames --untracked-files=all --ignored=matching 2>/dev/null || printf '\\000ignored_list_failed\\000'; } \\\n",
            "  | xargs -0 sh -c 'nl=\"\n\"; rc=0; for e; do case $e in ignored_list_failed) rc=1 ;; *\"$nl\"*|\"!! \"*/) ;; \"!! \"*) p=${e#\"!! \"}; if [ -f \"$p\" ] && [ ! -L \"$p\" ]; then printf \"./%s\\n\" \"$p\"; fi ;; esac; done; exit $rc' sh\n",
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
            // The paths in the file `$1` that the checkout at `.` ignores,
            // sorted, into `$2`. check-ignore leaves out tracked paths and
            // exits 1 when it ignores none; when it can't answer at all, `$2`
            // is empty and this fails.
            "ignored_check() {\n",
            "  tr '\\n' '\\000' < \"$1\" | G check-ignore -z --stdin > \"$2.z\" 2>/dev/null\n",
            "  _ic=$?\n",
            "  [ \"$_ic\" -le 1 ] || : > \"$2.z\"\n",
            "  tr '\\000' '\\n' < \"$2.z\" | LC_ALL=C sort > \"$2\"\n",
            "  rm -f -- \"$2.z\"\n",
            "  [ \"$_ic\" -le 1 ]\n",
            "}\n",
            // Place the archive `$ignored` (checked against
            // `$ignored_sha256`) into the checkout at `.`, with the excludes
            // list `$ignored_excludes` (checked against
            // `$ignored_excludes_sha256`) when there is one; see the module
            // documentation. Notes each line it adds to `info/exclude` in
            // `$ignored_excludes_journal`, when set, before adding it.
            // Prints `ignored_placed`, `ignored_excluded` when it added
            // entries to `info/exclude`, and for what it left out
            // `ignored_unignored` / `ignored_elsewhere` /
            // `ignored_in_the_way` with one path of each as an example. Uses
            // `fail` for anything it can't do.
            "ignored_place() {\n",
            "  [ \"$(sha256_of \"$ignored\")\" = \"$ignored_sha256\" ] || fail 'the ignored files do not match their checksum'\n",
            "  _st=\"$ignored.d\"\n",
            "  rm -rf -- \"$_st\"\n",
            "  mkdir -p -- \"$_st/files\" || fail \"cannot create $_st\"\n",
            "  _err=$(tar -xf \"$ignored\" -C \"$_st/files\" 2>&1) || fail \"cannot unpack the ignored files: $_err\"\n",
            "  (cd -- \"$_st/files\" && find . -type f) | LC_ALL=C sort > \"$_st/all\" || fail 'cannot list the ignored files'\n",
            "  : > \"$_st/local\"\n",
            "  if [ -n \"$ignored_excludes\" ]; then\n",
            "    [ \"$(sha256_of \"$ignored_excludes\")\" = \"$ignored_excludes_sha256\" ] || fail 'the ignored files to exclude do not match their checksum'\n",
            "    LC_ALL=C sort -u \"$ignored_excludes\" > \"$_st/local\" || fail 'cannot read the ignored files to exclude'\n",
            "  fi\n",
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
            // Then the source machine's own rules, for the free paths they
            // decided that this checkout doesn't ignore: one anchored entry
            // each, with the characters a pattern treats specially escaped,
            // appended unless the same line is there already. A local
            // destination's `info/exclude` is its shared clone's, which
            // usually ignores them already — and which every worktree of the
            // clone reads, so a path another one has a file or directory at
            // gets no entry (it would hide that one there too, from `git
            // add -A` and a later transfer's snapshot), and isn't placed. A
            // fresh workspace clone has no other worktree. When the
            // worktrees can't be listed, none gets an entry (and each is
            // left out as one this checkout doesn't ignore).
            "  LC_ALL=C comm -12 \"$_st/free\" \"$_st/local\" > \"$_st/local_free\"\n",
            "  if ignored_check \"$_st/local_free\" \"$_st/local_ignored\"; then\n",
            "    LC_ALL=C comm -23 \"$_st/local_free\" \"$_st/local_ignored\" > \"$_st/local_add\"\n",
            "  else\n",
            "    : > \"$_st/local_add\"\n",
            "  fi\n",
            "  : > \"$_st/elsewhere\"\n",
            "  if [ -s \"$_st/local_add\" ]; then\n",
            "    if G worktree list --porcelain > \"$_st/worktrees.raw\" 2>/dev/null; then\n",
            "      awk '/^worktree / { p = substr($0, 10); b = 0; next } $0 == \"bare\" { b = 1; next } $0 == \"\" { if (p != \"\" && !b) print p; p = \"\" } END { if (p != \"\" && !b) print p }' \"$_st/worktrees.raw\" > \"$_st/worktrees\"\n",
            "      _here=$(pwd -P)\n",
            "      while IFS= read -r _w; do\n",
            "        [ -d \"$_w\" ] || continue\n",
            "        [ \"$(unset CDPATH; cd -- \"$_w\" 2>/dev/null && pwd -P)\" != \"$_here\" ] || continue\n",
            "        while IFS= read -r _p; do\n",
            "          _r=${_p#./}\n",
            "          if [ -e \"$_w/$_r\" ] || [ -L \"$_w/$_r\" ]; then printf '%s\\n' \"$_p\" >> \"$_st/elsewhere\"; fi\n",
            "        done < \"$_st/local_add\"\n",
            "      done < \"$_st/worktrees\"\n",
            "    else\n",
            "      : > \"$_st/local_add\"\n",
            "    fi\n",
            "    LC_ALL=C sort -u \"$_st/elsewhere\" > \"$_st/elsewhere.sorted\" && mv -f -- \"$_st/elsewhere.sorted\" \"$_st/elsewhere\" || fail 'cannot note the ignored files to exclude'\n",
            "    LC_ALL=C comm -23 \"$_st/local_add\" \"$_st/elsewhere\" > \"$_st/local_own\" && mv -f -- \"$_st/local_own\" \"$_st/local_add\" || fail 'cannot note the ignored files to exclude'\n",
            "  fi\n",
            // The journal says, before each change is made, what file it is,
            // whether this made it, whether this ended its last line, and
            // each line this added: what a rollback takes back out.
            "  if [ -s \"$_st/local_add\" ]; then\n",
            "    _ex=$(G rev-parse --git-path info/exclude) || fail 'cannot find info/exclude'\n",
            "    case $_ex in /*) ;; *) _ex=$PWD/$_ex ;; esac\n",
            "    _jr=$ignored_excludes_journal\n",
            "    if [ -n \"$_jr\" ]; then\n",
            "      { printf 'file=%s\\n' \"$_ex\"; [ -e \"$_ex\" ] || printf 'created=1\\n'; } > \"$_jr\" || fail \"cannot write $_jr\"\n",
            "    fi\n",
            "    mkdir -p -- \"$(dirname -- \"$_ex\")\" || fail \"cannot create the directory for $_ex\"\n",
            "    if [ -s \"$_ex\" ] && [ -n \"$(tail -c 1 -- \"$_ex\")\" ]; then\n",
            "      [ -z \"$_jr\" ] || printf 'terminated=1\\n' >> \"$_jr\" || fail \"cannot write $_jr\"\n",
            "      printf '\\n' >> \"$_ex\" || fail \"cannot write $_ex\"\n",
            "    fi\n",
            "    while IFS= read -r _p; do\n",
            "      _e=/$(printf '%s' \"${_p#./}\" | LC_ALL=C sed 's/[][\\\\*?#! ]/\\\\&/g')\n",
            "      LC_ALL=C grep -qxF -e \"$_e\" -- \"$_ex\" 2>/dev/null && continue\n",
            "      [ -z \"$_jr\" ] || printf 'line=%s\\n' \"$_e\" >> \"$_jr\" || fail \"cannot write $_jr\"\n",
            "      printf '%s\\n' \"$_e\" >> \"$_ex\" || fail \"cannot write $_ex\"\n",
            "    done < \"$_st/local_add\"\n",
            "    printf 'ignored_excluded=%s\\n' \"$(ignored_count \"$_st/local_add\")\"\n",
            "  fi\n",
            // Then each free `.gitignore`, so what it ignores is ignored when
            // asked; the directories made for it are noted, to go again if
            // it does.
            "  : > \"$_st/tentative\"\n",
            "  : > \"$_st/made\"\n",
            "  while IFS= read -r _p; do\n",
            "    case $_p in */.gitignore) printf '%s\\n' \"$_p\" >> \"$_st/tentative\" ;; esac\n",
            "  done < \"$_st/free\"\n",
            "  while IFS= read -r _p; do\n",
            "    _r=${_p#./} _m=${_p%/.gitignore}\n",
            "    while [ \"$_m\" != . ] && [ ! -e \"$_m\" ]; do printf '%s\\n' \"$_m\" >> \"$_st/made\"; _m=${_m%/*}; done\n",
            "    mkdir -p -- \"${_p%/.gitignore}\" || fail \"cannot create the directory for $_r\"\n",
            "    cp -p \"$_st/files/$_r\" \"$_p\" || fail \"cannot place $_r\"\n",
            "    cmp -s \"$_st/files/$_r\" \"$_p\" || fail \"$_r did not arrive whole\"\n",
            "  done < \"$_st/tentative\"\n",
            // Then what this checkout ignores, until every `.gitignore` put
            // in place is among it: one that isn't would show up as
            // untracked, and what it alone ignored with it. If check-ignore
            // can't answer at all, nothing is placed.
            "  while :; do\n",
            "    ignored_check \"$_st/free\" \"$_st/ignored\"\n",
            "    LC_ALL=C comm -23 \"$_st/tentative\" \"$_st/ignored\" > \"$_st/drop\"\n",
            "    [ -s \"$_st/drop\" ] || break\n",
            "    while IFS= read -r _p; do rm -f -- \"$_p\"; done < \"$_st/drop\"\n",
            "    LC_ALL=C comm -23 \"$_st/tentative\" \"$_st/drop\" > \"$_st/kept\"\n",
            "    mv -f -- \"$_st/kept\" \"$_st/tentative\"\n",
            "  done\n",
            "  LC_ALL=C sort -ru \"$_st/made\" | while IFS= read -r _m; do rmdir -- \"$_m\" 2>/dev/null; done\n",
            "  LC_ALL=C comm -23 \"$_st/free\" \"$_st/ignored\" > \"$_st/left\"\n",
            "  LC_ALL=C comm -12 \"$_st/left\" \"$_st/elsewhere\" > \"$_st/elsewhere_left\"\n",
            "  LC_ALL=C comm -23 \"$_st/left\" \"$_st/elsewhere\" > \"$_st/unignored\"\n",
            "  LC_ALL=C comm -23 \"$_st/ignored\" \"$_st/tentative\" > \"$_st/rest\"\n",
            "  while IFS= read -r _p; do\n",
            "    _r=${_p#./}\n",
            "    case $_r in */*) [ -d \"./${_r%/*}\" ] || mkdir -p -- \"./${_r%/*}\" || fail \"cannot create the directory for $_r\" ;; esac\n",
            "    cp -p \"$_st/files/$_r\" \"./$_r\" || fail \"cannot place $_r\"\n",
            "    cmp -s \"$_st/files/$_r\" \"./$_r\" || fail \"$_r did not arrive whole\"\n",
            "  done < \"$_st/rest\"\n",
            "  printf 'ignored_placed=%s\\n' \"$(ignored_count \"$_st/ignored\")\"\n",
            "  if [ -s \"$_st/unignored\" ]; then\n",
            "    printf 'ignored_unignored=%s\\nignored_unignored_path=%s\\n' \"$(ignored_count \"$_st/unignored\")\" \"$(head -n 1 \"$_st/unignored\" | sed 's|^\\./||')\"\n",
            "  fi\n",
            "  if [ -s \"$_st/elsewhere_left\" ]; then\n",
            "    printf 'ignored_elsewhere=%s\\nignored_elsewhere_path=%s\\n' \"$(ignored_count \"$_st/elsewhere_left\")\" \"$(head -n 1 \"$_st/elsewhere_left\" | sed 's|^\\./||')\"\n",
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
/// directory is created), `$3` the cap in bytes, `$4` where to write the
/// excludes list.
///
/// Prints `ignored_files` and `ignored_bytes`. With none, that is all. Past
/// the cap it adds `ignored_over_cap=1` and writes nothing. Otherwise it
/// replaces `$2` whole — through `$2.part`, so an archive already there
/// stays until this one is complete — and prints its `ignored_size` and
/// `ignored_sha256`; and replaces `$4` with the `./<path>` lines of the ones
/// only rules outside the checkout's `.gitignore` files ignore, printing its
/// `ignored_excludes_size` and `ignored_excludes_sha256`, or removes it when
/// there are none (or check-ignore can't say). Reads the checkout only.
pub const IGNORED_ARCHIVE_SCRIPT: &str = concat!(
    prelude!(),
    ignored_prelude!(),
    "cd -- \"$1\" 2>/dev/null || fail \"cannot enter $1\"\n",
    "ar=$2 cap=$3 ex=$4\n",
    "case $cap in ''|*[!0-9]*) fail \"not a size: $cap\" ;; esac\n",
    "[ -n \"$ex\" ] || fail 'no path for the excludes list'\n",
    "top=$(G rev-parse --show-toplevel 2>/dev/null) || fail \"not a git checkout: $PWD\"\n",
    "cd -- \"$top\" || fail \"cannot enter $top\"\n",
    "mkdir -p -- \"$(dirname -- \"$ar\")\" || fail \"cannot create the directory for $ar\"\n",
    "list=\"$ar.list\"\n",
    "on_fail() { rm -f -- \"$list\" \"$ar.part\" \"$ex.z\" \"$ex.part\"; }\n",
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
    // What decided each one: check-ignore -v -z gives `<source> NUL <line>
    // NUL <pattern> NUL <path> NUL`, and names a `.gitignore` in the
    // checkout by its path from the top, `info/exclude` by a path into the
    // git directory, and the global excludes file by an absolute path. A
    // negated pattern means not ignored; none should decide here, and none
    // is listed.
    "rm -f -- \"$ex\"\n",
    "tr '\\n' '\\000' < \"$list\" | G check-ignore -v -z --stdin > \"$ex.z\" 2>/dev/null\n",
    "if [ $? -le 1 ]; then\n",
    "  tr '\\000' '\\n' < \"$ex.z\" | awk 'NR % 4 == 1 { s = $0 } NR % 4 == 3 { p = $0 } NR % 4 == 0 && p !~ /^!/ && (s ~ /^\\// || (s != \".gitignore\" && s !~ /\\/\\.gitignore$/)) { print }' | LC_ALL=C sort > \"$ex.part\"\n",
    "else\n",
    "  : > \"$ex.part\"\n",
    "fi\n",
    "rm -f -- \"$ex.z\"\n",
    "mv -f -- \"$ar.part\" \"$ar\" || fail \"cannot write $ar\"\n",
    "rm -f -- \"$list\"\n",
    "printf 'ignored_size=%s\\nignored_sha256=%s\\n' \"$(size_of \"$ar\")\" \"$(sha256_of \"$ar\")\"\n",
    "if [ -s \"$ex.part\" ]; then\n",
    "  mv -f -- \"$ex.part\" \"$ex\" || fail \"cannot write $ex\"\n",
    "  printf 'ignored_excludes_size=%s\\nignored_excludes_sha256=%s\\n' \"$(size_of \"$ex\")\" \"$(sha256_of \"$ex\")\"\n",
    "else\n",
    "  rm -f -- \"$ex.part\"\n",
    "fi\n",
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
    /// The excludes list beside it: the files in it that only rules outside
    /// the checkout's `.gitignore` files ignore, when there are any.
    pub excludes: Option<BundleFile>,
}

impl IgnoredFiles {
    /// The archive's file name for a bundle's: `0-widgets.bundle` keeps its
    /// ignored files in `0-widgets.ignored.tar`.
    pub fn archive_name(bundle_name: &str) -> String {
        let stem = bundle_name.strip_suffix(".bundle").unwrap_or(bundle_name);
        format!("{stem}.ignored.tar")
    }

    /// The excludes list's file name for a bundle's: `0-widgets.bundle`
    /// keeps it in `0-widgets.ignored.excludes`.
    pub fn excludes_name(bundle_name: &str) -> String {
        let stem = bundle_name.strip_suffix(".bundle").unwrap_or(bundle_name);
        format!("{stem}.ignored.excludes")
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
        let excludes = match report.get("ignored_excludes_sha256") {
            Some(_) if archive.is_some() => Some(BundleFile::from_report_as(
                STEP,
                &report,
                "ignored_excludes",
            )?),
            _ => None,
        };
        Ok(Self {
            files,
            bytes,
            over_cap,
            archive,
            excludes,
        })
    }
}

/// Arguments for [`IGNORED_ARCHIVE_SCRIPT`].
pub fn archive_args(checkout: &str, archive: &str, cap: u64, excludes: &str) -> Vec<String> {
    vec![
        checkout.to_string(),
        archive.to_string(),
        cap.to_string(),
        excludes.to_string(),
    ]
}

/// What placing an archive at the destination did: part of
/// [`ApplyReport`](super::ApplyReport).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoredPlacement {
    pub placed: u64,
    /// Files only the source machine's own excludes ignored, which this
    /// one's didn't, given an entry in its `info/exclude` so they could be
    /// placed.
    pub excluded: u64,
    /// Files the destination doesn't ignore (a `.gitignore` change that
    /// stayed behind, say), so they were left out rather than pass for the
    /// branch's own changes there.
    pub unignored: u64,
    pub unignored_example: Option<String>,
    /// Files only the source machine's own excludes ignored, given no entry
    /// in this one's `info/exclude` as another worktree of the repository
    /// has something at the path, which the entry would hide there too.
    pub elsewhere: u64,
    pub elsewhere_example: Option<String>,
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
            excluded: count("ignored_excluded"),
            unignored: count("ignored_unignored"),
            unignored_example: example("ignored_unignored_path"),
            elsewhere: count("ignored_elsewhere"),
            elsewhere_example: example("ignored_elsewhere_path"),
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
                excludes: None,
            }
        );
        let with_excludes = IgnoredFiles::parse(&format!(
            "ignored_files=3\nignored_bytes=120\nignored_size=10240\nignored_sha256={sha}\n\
             ignored_excludes_size=12\nignored_excludes_sha256={}\n",
            "CD".repeat(32)
        ))
        .unwrap();
        assert_eq!(
            with_excludes.excludes,
            Some(BundleFile {
                size: 12,
                sha256: "cd".repeat(32)
            })
        );
        // An excludes list promises its size.
        assert!(IgnoredFiles::parse(&format!(
            "ignored_files=3\nignored_bytes=120\nignored_size=10240\nignored_sha256={sha}\n\
             ignored_excludes_sha256={sha}\n"
        ))
        .is_err());
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
        assert_eq!(
            IgnoredFiles::excludes_name("0-widgets.bundle"),
            "0-widgets.ignored.excludes"
        );
    }

    #[test]
    fn a_placement_report_names_what_was_left_out() {
        let report = ScriptReport::parse(
            "ignored_placed=2\nignored_excluded=1\nignored_unignored=1\nignored_unignored_path=.envrc\n\
             ignored_elsewhere=2\nignored_elsewhere_path=NOTES.md\n\
             ignored_in_the_way=3\nignored_in_the_way_path=cache/x.log\n",
        );
        assert_eq!(
            IgnoredPlacement::from_report(&report),
            Some(IgnoredPlacement {
                placed: 2,
                excluded: 1,
                unignored: 1,
                unignored_example: Some(".envrc".to_string()),
                elsewhere: 2,
                elsewhere_example: Some("NOTES.md".to_string()),
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
