//! The shell scripts a transfer runs at either end, and the Rust side of their
//! contract: argument builders and parsers for their `key=value` output.
//!
//! Everything a transfer does to a repository happens in these scripts, so a
//! step on a Blox workspace is one `ws exec` round trip, and the same text runs
//! locally and remotely. Blox's `/bin/sh` is dash, so they stay POSIX: no
//! `local`, no arrays, no `[[`, no `pipefail`, no prefix assignments on shell
//! functions. Every git call goes through `G`, which turns hooks off — Blox
//! clones install lefthook, and a transfer must never run project code — and
//! runs in the C locale, so the git messages a script matches on read the same
//! whatever the user's `LANG`. The snapshot is built with plumbing in a copy of
//! the index, so the source's working tree, index and branch are never written.
//!
//! The working state travels as two commits on top of `HEAD`:
//!
//! - `I` (`index_commit`): the index, `commit-tree $(write-tree) -p HEAD`.
//! - `W` (`snapshot`): the working tree including untracked files, built from a
//!   copy of the index after `add -A`, with `I` as its parent.
//!
//! `W`'s history therefore carries `I` and `HEAD`, so the bundle only needs the
//! one temporary ref `refs/staged-transfer/<id>/snapshot`. The destination
//! restores the working tree with `read-tree -u --reset W` and then the index
//! with `read-tree --reset W^`, and checks both tree hashes.
//!
//! Failures print `error=<message>` and exit non-zero; a destination missing
//! commits the bundle was cut against also prints
//! `error_kind=missing_prerequisites`, so the caller can re-bundle without
//! exclusions.

use super::TransferError;

/// Definitions every script starts with.
macro_rules! prelude {
    () => {
        concat!(
            "set -u\n",
            // A prefix assignment on an external command is plain POSIX.
            "G() { LC_ALL=C git -c core.hooksPath=/dev/null \"$@\"; }\n",
            // `G` against another index file. A prefix assignment on a shell
            // function is unspecified in POSIX, so this calls git directly.
            "GI() { _gi_index=$1; shift; GIT_INDEX_FILE=$_gi_index LC_ALL=C git -c core.hooksPath=/dev/null \"$@\"; }\n",
            // `on_fail` is a script's chance to undo what it has done so far.
            "on_fail() { :; }\n",
            "fail() { on_fail; printf 'error=%s\\n' \"$(printf '%s' \"$*\" | tr '\\n' ' ')\"; exit 1; }\n",
            "sha256_of() {\n",
            "  if command -v sha256sum >/dev/null 2>&1; then sha256sum < \"$1\"\n",
            "  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 < \"$1\"\n",
            "  else openssl dgst -sha256 -r < \"$1\"; fi | cut -d ' ' -f 1\n",
            "}\n",
            "size_of() { wc -c < \"$1\" | tr -d ' \\t'; }\n",
            // Hex-encode stdin, so `git config -z` output (values may hold
            // newlines) survives a line-oriented report.
            "hex() { od -An -v -tx1 | tr -d ' \\t\\n'; }\n",
            // Escape the ERE metacharacters a ref name can contain, for
            // `git config --get-regexp`.
            "esc() { printf '%s' \"$1\" | sed 's/[.$+(){}|]/\\\\&/g'; }\n",
        )
    };
}

/// Snapshot a checkout and bundle the snapshot.
///
/// Arguments: `$1` the checkout, `$2` the branch it must have checked out,
/// `$3` the bundle to write (absolute; its directory is created), `$4` the
/// transfer id, `$5…` commits the destination already has (its origin tips).
/// Tips this repository also has are excluded from the bundle.
///
/// Refuses a detached `HEAD`, the wrong branch, an unborn branch, unresolved
/// conflicts, and an in-progress merge, rebase, cherry-pick, revert or bisect.
/// Prints the commits and trees of the snapshot, the bundle's size and
/// sha256, the branch's `branch.<b>.*` config, and the `url`/`pushurl`/`fetch`
/// of any non-origin remote the branch pulls from or pushes to.
pub const SNAPSHOT_SCRIPT: &str = concat!(
    prelude!(),
    "cd -- \"$1\" 2>/dev/null || fail \"cannot enter $1\"\n",
    "branch=$2 bundle=$3 tid=$4\n",
    "shift 4\n",
    "top=$(G rev-parse --show-toplevel 2>/dev/null) || fail \"not a git checkout: $PWD\"\n",
    "cd -- \"$top\" || fail \"cannot enter $top\"\n",
    // --- Refuse what a snapshot could not carry faithfully ---
    "head_ref=$(G symbolic-ref -q HEAD) || fail 'HEAD is detached'\n",
    "[ \"$head_ref\" = \"refs/heads/$branch\" ] || fail \"HEAD is on ${head_ref#refs/heads/}, not $branch\"\n",
    "for marker in MERGE_HEAD:merge CHERRY_PICK_HEAD:cherry-pick REVERT_HEAD:revert BISECT_LOG:bisect rebase-merge:rebase rebase-apply:rebase; do\n",
    "  [ -e \"$(G rev-parse --git-path \"${marker%%:*}\")\" ] && fail \"a ${marker#*:} is in progress\"\n",
    "done\n",
    "[ -z \"$(G ls-files -u)\" ] || fail 'the index has unresolved conflicts'\n",
    "head=$(G rev-parse -q --verify 'HEAD^{commit}') || fail \"$branch has no commits\"\n",
    // Plumbing commits need an identity, and must not prompt to sign.
    "export GIT_AUTHOR_NAME=Staged GIT_AUTHOR_EMAIL=staged@localhost GIT_COMMITTER_NAME=Staged GIT_COMMITTER_EMAIL=staged@localhost\n",
    "mkdir -p -- \"$(dirname -- \"$bundle\")\" || fail \"cannot create the directory for $bundle\"\n",
    // --- I and W, built in a copy of the index ---
    // Copying keeps the stat cache, so `add -A` only hashes what changed, and
    // the real index is never locked or rewritten. `-p` keeps the index's
    // mtime, which git's racy-entry check compares entries against: with a
    // fresh mtime, a file rewritten at the same size in the same second as the
    // index was written would look clean, and its change would be left out.
    "tmp_index=\"$bundle.index\"\n",
    "rm -f -- \"$tmp_index\"\n",
    "real_index=$(G rev-parse --git-path index)\n",
    "if [ -f \"$real_index\" ]; then cp -p -- \"$real_index\" \"$tmp_index\" || fail 'cannot copy the index'; fi\n",
    "tree_i=$(GI \"$tmp_index\" write-tree) || { rm -f -- \"$tmp_index\"; fail 'cannot write the index tree'; }\n",
    "err=$(GI \"$tmp_index\" add -A 2>&1) || { rm -f -- \"$tmp_index\"; fail \"cannot stage the working tree: $err\"; }\n",
    "tree_w=$(GI \"$tmp_index\" write-tree) || { rm -f -- \"$tmp_index\"; fail 'cannot write the working tree'; }\n",
    "rm -f -- \"$tmp_index\"\n",
    "index_commit=$(printf 'staged transfer %s: index\\n' \"$tid\" | G commit-tree --no-gpg-sign \"$tree_i\" -p \"$head\") || fail 'cannot commit the index'\n",
    "snapshot=$(printf 'staged transfer %s: working tree\\n' \"$tid\" | G commit-tree --no-gpg-sign \"$tree_w\" -p \"$index_commit\") || fail 'cannot commit the working tree'\n",
    "snap_ref=\"refs/staged-transfer/$tid/snapshot\"\n",
    "G update-ref \"$snap_ref\" \"$snapshot\" || fail \"cannot write $snap_ref\"\n",
    // --- The bundle, without what the destination already has ---
    // The snapshot commits are always new, so the bundle is never empty, even
    // for a clean branch whose HEAD is one of the excluded tips.
    "not=''\n",
    "for tip; do\n",
    "  case $tip in ''|*[!0-9a-f]*) continue ;; esac\n",
    "  G cat-file -e \"$tip^{commit}\" 2>/dev/null && not=\"$not $tip\"\n",
    "done\n",
    "rm -f -- \"$bundle\"\n",
    "err=$(G bundle create \"$bundle\" \"$snap_ref\" ${not:+--not} $not 2>&1) || fail \"git bundle create failed: $err\"\n",
    "printf 'head=%s\\nhead_tree=%s\\n' \"$head\" \"$(G rev-parse \"$head^{tree}\")\"\n",
    "printf 'index_commit=%s\\ntree_i=%s\\n' \"$index_commit\" \"$tree_i\"\n",
    "printf 'snapshot=%s\\ntree_w=%s\\n' \"$snapshot\" \"$tree_w\"\n",
    "printf 'excluded=%s\\n' \"$(printf '%s' \"$not\" | wc -w | tr -d ' \\t')\"\n",
    "printf 'bundle_size=%s\\nbundle_sha256=%s\\n' \"$(size_of \"$bundle\")\" \"$(sha256_of \"$bundle\")\"\n",
    // --- What the destination needs to track the same upstream ---
    "printf 'branch_config=%s\\n' \"$(G config --local -z --get-regexp \"^branch\\\\.$(esc \"$branch\")\\\\.\" | hex)\"\n",
    "seen=' origin . '\n",
    "for key in remote pushremote; do\n",
    "  r=$(G config --local --get \"branch.$branch.$key\" 2>/dev/null) || continue\n",
    "  [ -n \"$r\" ] || continue\n",
    "  case $seen in *\" $r \"*) continue ;; esac\n",
    "  seen=\"$seen$r \"\n",
    "  printf 'remote_config=%s\\n' \"$(G config --local -z --get-regexp \"^remote\\\\.$(esc \"$r\")\\\\.(url|pushurl|fetch)\\$\" | hex)\"\n",
    "done\n",
    // --- What a bundle does not carry ---
    "[ -f .gitmodules ] && printf 'warning=submodules\\n'\n",
    "G grep -q -e 'filter=lfs' -- '*.gitattributes' 2>/dev/null && printf 'warning=lfs\\n'\n",
    "exit 0\n",
);

/// Re-bundle an existing snapshot with no exclusions, for a destination that
/// turned out to lack the commits the first bundle was cut against.
///
/// Arguments: `$1` the checkout, `$2` the bundle to write, `$3` the transfer id.
pub const REBUNDLE_SCRIPT: &str = concat!(
    prelude!(),
    "cd -- \"$1\" 2>/dev/null || fail \"cannot enter $1\"\n",
    "bundle=$2 snap_ref=\"refs/staged-transfer/$3/snapshot\"\n",
    "G rev-parse -q --verify \"$snap_ref^{commit}\" >/dev/null || fail \"$snap_ref is gone\"\n",
    "rm -f -- \"$bundle\"\n",
    "err=$(G bundle create \"$bundle\" \"$snap_ref\" 2>&1) || fail \"git bundle create failed: $err\"\n",
    "printf 'bundle_size=%s\\nbundle_sha256=%s\\n' \"$(size_of \"$bundle\")\" \"$(sha256_of \"$bundle\")\"\n",
    "exit 0\n",
);

/// Check, without writing to them, that checkouts are still what their
/// snapshots captured — before they are deleted on the strength of those
/// snapshots. One run checks every checkout it is given, so a workspace's
/// are all checked in one exec, right before the workspace is deleted.
///
/// Arguments: six per checkout — the checkout, the branch it must have
/// checked out, the snapshot's `head`, `tree_i` and `tree_w`, and a scratch
/// file (outside the checkout) for the copy of the index.
///
/// For each it prints `repo=<index>`, then `unchanged=1`, `changed=<what>` for
/// the first difference, or `error=<message>` when it could not tell. The
/// index and working tree are read the way [`SNAPSHOT_SCRIPT`] read them, in a
/// copy of the index, so not even the real index's stat cache is touched.
pub const VERIFY_SCRIPT: &str = concat!(
    prelude!(),
    // A subshell per checkout, so `fail` and `changed` end that one's check.
    "check() (\n",
    "  cd -- \"$1\" 2>/dev/null || fail \"cannot enter $1\"\n",
    "  branch=$2 head=$3 tree_i=$4 tree_w=$5 tmp_index=$6\n",
    "  top=$(G rev-parse --show-toplevel 2>/dev/null) || fail \"not a git checkout: $PWD\"\n",
    "  cd -- \"$top\" || fail \"cannot enter $top\"\n",
    "  on_fail() { rm -f -- \"$tmp_index\"; }\n",
    "  changed() { rm -f -- \"$tmp_index\"; printf 'changed=%s\\n' \"$*\"; exit 0; }\n",
    "  [ \"$(G symbolic-ref -q HEAD)\" = \"refs/heads/$branch\" ] || changed \"$branch is no longer checked out\"\n",
    "  now=$(G rev-parse -q --verify 'HEAD^{commit}')\n",
    "  [ \"$now\" = \"$head\" ] || changed \"HEAD moved from $head to ${now:-nothing}\"\n",
    "  mkdir -p -- \"$(dirname -- \"$tmp_index\")\" || fail \"cannot create the directory for $tmp_index\"\n",
    "  rm -f -- \"$tmp_index\"\n",
    "  real_index=$(G rev-parse --git-path index)\n",
    "  if [ -f \"$real_index\" ]; then cp -p -- \"$real_index\" \"$tmp_index\" || fail 'cannot copy the index'; fi\n",
    "  t=$(GI \"$tmp_index\" write-tree) || fail 'cannot read the index'\n",
    "  [ \"$t\" = \"$tree_i\" ] || changed 'the index changed'\n",
    "  err=$(GI \"$tmp_index\" add -A 2>&1) || fail \"cannot read the working tree: $err\"\n",
    "  t=$(GI \"$tmp_index\" write-tree) || fail 'cannot read the working tree'\n",
    "  [ \"$t\" = \"$tree_w\" ] || changed 'files changed'\n",
    "  rm -f -- \"$tmp_index\"\n",
    "  printf 'unchanged=1\\n'\n",
    ")\n",
    "i=0\n",
    "while [ $# -ge 6 ]; do\n",
    "  printf 'repo=%s\\n' \"$i\"\n",
    "  check \"$1\" \"$2\" \"$3\" \"$4\" \"$5\" \"$6\"\n",
    "  i=$((i + 1))\n",
    "  shift 6\n",
    "done\n",
    "exit 0\n",
);

/// Fetch branches from origin at the destination and report their tips, so
/// the source can leave out of its bundle what the destination already has.
///
/// Arguments: `$1` the destination repository, `$2…` branch names on origin
/// (the base, and the branch's upstream). A name origin does not have is
/// reported as `unfetched=<name>`; a tip is reported for every name the
/// repository has a remote-tracking ref for, fetched or not.
pub const ORIGIN_TIPS_SCRIPT: &str = concat!(
    prelude!(),
    "cd -- \"$1\" 2>/dev/null || fail \"cannot enter $1\"\n",
    "shift\n",
    "for name; do\n",
    "  [ -n \"$name\" ] || continue\n",
    "  G fetch -q --no-tags origin \"+refs/heads/$name:refs/remotes/origin/$name\" >/dev/null 2>&1 || printf 'unfetched=%s\\n' \"$name\"\n",
    "  tip=$(G rev-parse -q --verify \"refs/remotes/origin/$name^{commit}\") && printf 'tip=%s\\n' \"$tip\"\n",
    "done\n",
    "exit 0\n",
);

/// Land a bundle at the destination, and/or finish it in a checkout.
///
/// Named arguments (`key=value`), then `--` and the config records described
/// on [`ApplyRequest::args`]. `mode` is one of:
///
/// - `checkout`: verify and fetch the bundle, check the branch out in `dir`
///   with `checkout -B`, write its config, then finish. For a Blox clone,
///   where everything happens in one exec.
/// - `branch`: verify and fetch the bundle and create `refs/heads/<branch>`
///   in the repository at `dir` without touching any checkout, then write its
///   config. For the shared local clone; the caller then adds a worktree and
///   runs `finish` in it.
/// - `finish`: in a checkout already on the branch, optionally restore the
///   uncommitted state (`restore=1`), verify, and drop the temporary ref.
///
/// The branch's config is replaced wholesale, so a branch the source tracked
/// nothing for tracks nothing here. A non-origin remote the source used is
/// added when the destination lacks it (`added_remote=<name>`), and a
/// `warning=` says so when the destination's has another URL; the upstream's
/// remote-tracking ref is fetched when it is missing, so ahead/behind counts
/// come out the same.
///
/// `branch` creates the branch only when no ref has the name, atomically, and
/// says so with `created_branch=1`; a branch already at `head` is reused. A
/// failure after that undoes the branch it created and the remotes it added,
/// so what the caller rolls back is only what a successful run reported.
pub const APPLY_SCRIPT: &str = concat!(
    prelude!(),
    "mode='' dir='' bundle='' sha256='' tid='' branch='' head='' snapshot='' tree_i='' tree_w='' restore=0\n",
    "while [ $# -gt 0 ]; do\n",
    "  case $1 in\n",
    "    --) shift; break ;;\n",
    "    mode=*) mode=${1#mode=} ;;\n",
    "    dir=*) dir=${1#dir=} ;;\n",
    "    bundle=*) bundle=${1#bundle=} ;;\n",
    "    sha256=*) sha256=${1#sha256=} ;;\n",
    "    tid=*) tid=${1#tid=} ;;\n",
    "    branch=*) branch=${1#branch=} ;;\n",
    "    head=*) head=${1#head=} ;;\n",
    "    snapshot=*) snapshot=${1#snapshot=} ;;\n",
    "    tree_i=*) tree_i=${1#tree_i=} ;;\n",
    "    tree_w=*) tree_w=${1#tree_w=} ;;\n",
    "    restore=*) restore=${1#restore=} ;;\n",
    "    *) fail \"unknown argument: $1\" ;;\n",
    "  esac\n",
    "  shift\n",
    "done\n",
    "case $mode in checkout|branch|finish) ;; *) fail \"unknown mode: $mode\" ;; esac\n",
    "cd -- \"$dir\" 2>/dev/null || fail \"cannot enter $dir\"\n",
    "top=$(G rev-parse --show-toplevel 2>/dev/null) || fail \"not a git checkout: $PWD\"\n",
    "cd -- \"$top\" || fail \"cannot enter $top\"\n",
    "snap_ref=\"refs/staged-transfer/$tid/snapshot\"\n",
    "if [ \"$mode\" != finish ]; then\n",
    // --- The bytes, then the commits the bundle was cut against ---
    "  [ \"$(sha256_of \"$bundle\")\" = \"$sha256\" ] || fail 'the bundle does not match its checksum'\n",
    "  if ! err=$(G bundle verify \"$bundle\" 2>&1 >/dev/null); then\n",
    "    case $err in *prerequisite*) printf 'error_kind=missing_prerequisites\\n' ;; esac\n",
    "    fail \"git bundle verify failed: $err\"\n",
    "  fi\n",
    "  err=$(G fetch -q --no-tags --no-write-fetch-head \"$bundle\" \"+$snap_ref:$snap_ref\" 2>&1) || fail \"cannot fetch from the bundle: $err\"\n",
    "  w=$(G rev-parse -q --verify \"$snap_ref^{commit}\") || fail \"the bundle has no $snap_ref\"\n",
    "  [ \"$w\" = \"$snapshot\" ] || fail \"the bundle carries $w, not $snapshot\"\n",
    "  [ \"$(G rev-parse \"$w^1^1\")\" = \"$head\" ] || fail \"the snapshot is not based on $head\"\n",
    "  [ \"$(G rev-parse \"$w^{tree}\")\" = \"$tree_w\" ] || fail 'the working tree snapshot is corrupt'\n",
    "  [ \"$(G rev-parse \"$w^1^{tree}\")\" = \"$tree_i\" ] || fail 'the index snapshot is corrupt'\n",
    // --- The branch ---
    // What a failure from here on takes back. Remote names are refname
    // components, so they hold no spaces or glob characters.
    "  created=0 added=''\n",
    "  on_fail() {\n",
    "    for r in $added; do G config --local --remove-section \"remote.$r\" >/dev/null 2>&1; done\n",
    "    if [ \"$created\" = 1 ]; then\n",
    "      G update-ref -d \"refs/heads/$branch\" \"$head\" >/dev/null 2>&1\n",
    "      G config --local --remove-section \"branch.$branch\" >/dev/null 2>&1\n",
    "    fi\n",
    "  }\n",
    "  if [ \"$mode\" = checkout ]; then\n",
    "    [ -z \"$(G status --porcelain --untracked-files=no)\" ] || fail 'the destination checkout has uncommitted changes'\n",
    // An untracked file the landing would write over is refused. Any other
    // (a workspace's bootstrap leaves some) stays, and a warning says so, as
    // it would otherwise pass for the branch's own.
    "    n=$(G ls-files --others --exclude-standard | wc -l | tr -d ' \\t')\n",
    "    if [ \"$n\" != 0 ]; then\n",
    "      trees=$head\n",
    "      [ \"$restore\" = 1 ] && trees=\"$trees $tree_w\"\n",
    "      for t in $trees; do\n",
    "        clash=$(G ls-files -z --others --exclude-standard | xargs -0 env LC_ALL=C git -c core.hooksPath=/dev/null -c core.quotePath=false --literal-pathspecs ls-tree -r --name-only \"$t\" -- | head -n 1)\n",
    "        [ -z \"$clash\" ] || fail \"the destination checkout has an untracked $clash, which the branch would write over\"\n",
    "      done\n",
    "      if [ \"$n\" = 1 ]; then what='1 untracked file was'; else what=\"$n untracked files were\"; fi\n",
    "      printf 'warning=%s already in the destination checkout (%s, for one) and stayed there\\n' \"$what\" \"$(G -c core.quotePath=false ls-files --others --exclude-standard | head -n 1)\"\n",
    "    fi\n",
    "    err=$(G checkout -q --no-track -B \"$branch\" \"$head\" 2>&1) || fail \"cannot check out $branch: $err\"\n",
    "  else\n",
    "    cur=$(G rev-parse -q --verify \"refs/heads/$branch\")\n",
    "    if [ -z \"$cur\" ]; then\n",
    // An empty old value makes this a create: it fails if the branch has
    // appeared since the check.
    "      err=$(G update-ref \"refs/heads/$branch\" \"$head\" '' 2>&1) || fail \"cannot create $branch: $err\"\n",
    "      created=1\n",
    "      printf 'created_branch=1\\n'\n",
    "    elif [ \"$cur\" != \"$head\" ]; then\n",
    "      fail \"$branch already exists at $cur\"\n",
    "    fi\n",
    "  fi\n",
    // --- Its config, replaced wholesale ---
    "  G config --local --remove-section \"branch.$branch\" >/dev/null 2>&1\n",
    "  rname='' rskip=1 rurl=''\n",
    "  while [ $# -gt 0 ]; do\n",
    "    case $1 in\n",
    "      b) [ $# -ge 3 ] || fail 'truncated branch config'\n",
    "         G config --local --add \"branch.$branch.$2\" \"$3\" || fail \"cannot set branch.$branch.$2\"\n",
    "         shift 3 ;;\n",
    "      R) [ $# -ge 2 ] || fail 'truncated remote'\n",
    "         rname=$2 rskip=0\n",
    "         if rurl=$(G config --local --get \"remote.$rname.url\" 2>/dev/null); then\n",
    "           rskip=1\n",
    "         else\n",
    "           printf 'added_remote=%s\\n' \"$rname\"\n",
    "           added=\"$added $rname\"\n",
    "         fi\n",
    "         shift 2 ;;\n",
    "      r) [ $# -ge 3 ] || fail 'truncated remote config'\n",
    "         if [ \"$rskip\" = 0 ]; then\n",
    "           G config --local --add \"remote.$rname.$2\" \"$3\" || fail \"cannot set remote.$rname.$2\"\n",
    "         elif [ \"$2\" = url ] && [ \"$3\" != \"$rurl\" ]; then\n",
    "           printf 'warning=remote %s here points at %s, not %s as at the source; %s tracks it as it is here\\n' \"$rname\" \"$rurl\" \"$3\" \"$branch\"\n",
    "         fi\n",
    "         shift 3 ;;\n",
    "      *) fail \"unknown config record: $1\" ;;\n",
    "    esac\n",
    "  done\n",
    // --- The upstream's remote-tracking ref, so ahead/behind match ---
    "  up_remote=$(G config --get \"branch.$branch.remote\")\n",
    "  up_merge=$(G config --get \"branch.$branch.merge\")\n",
    "  if [ -n \"$up_remote\" ] && [ \"$up_remote\" != . ] && [ -n \"$up_merge\" ]; then\n",
    "    up_ref=$(G for-each-ref --format='%(upstream)' \"refs/heads/$branch\")\n",
    "    if [ -z \"$up_ref\" ] || ! G rev-parse -q --verify \"$up_ref\" >/dev/null; then\n",
    "      G fetch -q --no-tags --no-write-fetch-head \"$up_remote\" \"$up_merge\" >/dev/null 2>&1 || printf 'warning=cannot fetch %s from %s\\n' \"$up_merge\" \"$up_remote\"\n",
    "    fi\n",
    "  fi\n",
    "fi\n",
    "if [ \"$mode\" != branch ]; then\n",
    // --- Finish in the checkout ---
    "  [ \"$(G symbolic-ref -q HEAD)\" = \"refs/heads/$branch\" ] || fail \"the checkout is not on $branch\"\n",
    "  [ \"$(G rev-parse -q --verify HEAD)\" = \"$head\" ] || fail \"the checkout is not at $head\"\n",
    "  if [ \"$restore\" = 1 ]; then\n",
    "    w=$(G rev-parse -q --verify \"$snap_ref^{commit}\") || fail \"$snap_ref is gone\"\n",
    "    [ \"$w\" = \"$snapshot\" ] || fail \"$snap_ref moved\"\n",
    // The working tree and index become W; then the index alone goes back to
    // I, which leaves W's extra content as unstaged and untracked changes.
    "    err=$(G read-tree -u --reset \"$w\" 2>&1) || fail \"cannot restore the working tree: $err\"\n",
    "    G update-index -q --refresh >/dev/null 2>&1\n",
    "    [ \"$(G write-tree)\" = \"$tree_w\" ] && G diff-files --quiet || fail 'the restored working tree does not match the snapshot'\n",
    "    err=$(G read-tree --reset \"$w^1\" 2>&1) || fail \"cannot restore the index: $err\"\n",
    "    [ \"$(G write-tree)\" = \"$tree_i\" ] || fail 'the restored index does not match the snapshot'\n",
    "    G update-index -q --refresh >/dev/null 2>&1\n",
    "  fi\n",
    "  G update-ref -d \"$snap_ref\" >/dev/null 2>&1\n",
    "fi\n",
    "printf 'applied=%s\\n' \"$head\"\n",
    "exit 0\n",
);

/// Clone a repository unless it is already there — a transfer's secondary
/// repos at a fresh workspace, whose start only cloned the primary one — and
/// check a branch can land in it. A workspace's bootstrap can leave tracked
/// files changed in the clone `ws start` made, which the apply would only
/// refuse after the snapshot and the upload.
///
/// Arguments: `$1` the clone directory, `$2` the URL to clone from.
pub const CLONE_SCRIPT: &str = concat!(
    prelude!(),
    "if [ ! -d \"$1/.git\" ]; then\n",
    "  mkdir -p -- \"$(dirname -- \"$1\")\" || fail \"cannot create the directory for $1\"\n",
    "  err=$(G clone -q -- \"$2\" \"$1\" 2>&1) || fail \"cannot clone $2: $err\"\n",
    "fi\n",
    "cd -- \"$1\" 2>/dev/null || fail \"cannot enter $1\"\n",
    "dirty=$(G -c core.quotePath=false status --porcelain --untracked-files=no | cut -c4- | head -n 3 | paste -s -d ' ' -)\n",
    "[ -z \"$dirty\" ] || fail \"$1 already has changes to tracked files ($dirty), made in the workspace (by its bootstrap, say); the branch can't be checked out over them\"\n",
    "exit 0\n",
);

/// Inspect checkouts before a transfer, without writing to any of them.
///
/// Arguments: pairs of `dir branch`, one per repository. For each pair it
/// prints `repo=<index>` and then what it found: `missing=1` (no such
/// directory) or `not_a_checkout=1` and nothing else; otherwise `head_ref`
/// (or `detached=1`), `head` (absent on an unborn branch), any
/// `operation=` in progress, `staged`/`unstaged`/`untracked`/`conflicted`
/// counts, a `nested_repo=<path>` per untracked git repository inside the
/// checkout (which a snapshot could only record as a commit id, and which
/// `untracked` leaves out), `untracked_bytes` when there are untracked files,
/// `ignored` (ignored files outside ignored directories, each counted) and
/// `ignored_dirs` (directories an ignore pattern matches, each counted once
/// however much they hold) — none of which is carried — `repo_kib` (the
/// object store's size), `unpushed` (commits on no remote-tracking ref),
/// `upstream`, and the same `warning=`s as
/// [`SNAPSHOT_SCRIPT`]. `status` runs with `--no-optional-locks`, so even the
/// index's stat cache is left alone. `$branch` is only echoed back; the caller
/// compares `head_ref` against it.
pub const PREFLIGHT_SCRIPT: &str = concat!(
    prelude!(),
    "i=0\n",
    "while [ $# -ge 2 ]; do\n",
    "  dir=$1 branch=$2\n",
    "  shift 2\n",
    "  printf 'repo=%s\\nbranch=%s\\n' \"$i\" \"$branch\"\n",
    "  i=$((i + 1))\n",
    "  cd -- \"$dir\" 2>/dev/null || { printf 'missing=1\\n'; continue; }\n",
    "  top=$(G rev-parse --show-toplevel 2>/dev/null) && cd -- \"$top\" 2>/dev/null || { printf 'not_a_checkout=1\\n'; continue; }\n",
    "  if head_ref=$(G symbolic-ref -q HEAD); then printf 'head_ref=%s\\n' \"${head_ref#refs/heads/}\"; else printf 'detached=1\\n'; fi\n",
    "  head=$(G rev-parse -q --verify 'HEAD^{commit}') && printf 'head=%s\\n' \"$head\"\n",
    "  for marker in MERGE_HEAD:merge CHERRY_PICK_HEAD:cherry-pick REVERT_HEAD:revert BISECT_LOG:bisect rebase-merge:rebase rebase-apply:rebase; do\n",
    "    [ -e \"$(G rev-parse --git-path \"${marker%%:*}\")\" ] && printf 'operation=%s\\n' \"${marker#*:}\"\n",
    "  done\n",
    // Porcelain v1 is one line per path (odd names come quoted), `XY path`:
    // `??` untracked, `!!` ignored, an unmerged pair conflicted, else X
    // staged and Y not. With every untracked file listed, the only untracked
    // directories left are the ones git won't descend into: nested
    // repositories. `--ignored=matching` lists a directory an ignore pattern
    // matches (`node_modules/`) once, without walking it, and the files of an
    // untracked directory holding nothing but ignored files one by one.
    "  counts=$(G --no-optional-locks status --porcelain=v1 --untracked-files=all --ignored=matching 2>/dev/null | awk '\n",
    "    { xy = substr($0, 1, 2); dir = /\\/\"?$/ }\n",
    "    xy == \"!!\" { if (dir) igd++; else ig++; next }\n",
    "    xy == \"??\" && dir { print \"nested_repo=\" substr($0, 4); next }\n",
    "    xy == \"??\" { u++; next }\n",
    "    xy ~ /^(DD|AU|UD|UA|DU|AA|UU)$/ { c++; next }\n",
    "    { if (substr(xy, 1, 1) != \" \") s++; if (substr(xy, 2, 1) != \" \") w++ }\n",
    "    END { printf \"staged=%d\\nunstaged=%d\\nuntracked=%d\\nconflicted=%d\\nignored=%d\\nignored_dirs=%d\\n\", s, w, u, c, ig, igd }')\n",
    "  printf '%s\\n' \"$counts\"\n",
    "  case $counts in\n",
    "    *untracked=0*) ;;\n",
    "    *) printf 'untracked_bytes=%s\\n' \"$(G ls-files -z --others --exclude-standard | xargs -0 ls -lnd -- 2>/dev/null | awk '{ s += $5 } END { print s + 0 }')\" ;;\n",
    "  esac\n",
    "  printf 'repo_kib=%s\\n' \"$(G count-objects -v 2>/dev/null | awk '$1 == \"size:\" || $1 == \"size-pack:\" { s += $2 } END { print s + 0 }')\"\n",
    "  if [ -n \"${head:-}\" ]; then\n",
    "    printf 'unpushed=%s\\n' \"$(G rev-list --count HEAD --not --remotes 2>/dev/null)\"\n",
    "    up=$(G rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null) && printf 'upstream=%s\\n' \"$up\"\n",
    "  fi\n",
    "  head=''\n",
    "  [ -f .gitmodules ] && printf 'warning=submodules\\n'\n",
    "  G grep -q -e 'filter=lfs' -- '*.gitattributes' 2>/dev/null && printf 'warning=lfs\\n'\n",
    "done\n",
    "exit 0\n",
);

/// Remove a transfer's temporary refs, and optionally its scratch directory.
///
/// Arguments: `$1` the repository (skipped when it does not exist), `$2` the
/// transfer id, `$3` a scratch directory to delete, or empty to keep it.
pub const CLEANUP_SCRIPT: &str = concat!(
    prelude!(),
    "if cd -- \"$1\" 2>/dev/null; then\n",
    "  G for-each-ref --format='%(refname)' \"refs/staged-transfer/$2/\" | while IFS= read -r ref; do G update-ref -d \"$ref\"; done\n",
    "fi\n",
    "[ -z \"$3\" ] || rm -rf -- \"$3\"\n",
    "exit 0\n",
);

// =============================================================================
// Output
// =============================================================================

/// A script's `key=value` lines, in order. Lines without `=` are ignored.
#[derive(Debug, Default, Clone)]
pub struct ScriptReport {
    entries: Vec<(String, String)>,
}

impl ScriptReport {
    pub fn parse(stdout: &str) -> Self {
        let entries = stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self { entries }
    }

    /// The first value for `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value for `key`, in order.
    pub fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.entries
            .iter()
            .filter(move |(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn require(&self, step: &'static str, key: &str) -> Result<&str, TransferError> {
        self.get(key)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| TransferError::Protocol {
                step,
                message: format!("missing {key}"),
            })
    }

    fn require_oid(&self, step: &'static str, key: &str) -> Result<String, TransferError> {
        let value = self.require(step, key)?;
        if !is_object_id(value) {
            return Err(TransferError::Protocol {
                step,
                message: format!("{key} is not an object id: {value}"),
            });
        }
        Ok(value.to_string())
    }

    fn require_u64(&self, step: &'static str, key: &str) -> Result<u64, TransferError> {
        let value = self.require(step, key)?;
        value.parse().map_err(|_| TransferError::Protocol {
            step,
            message: format!("{key} is not a number: {value}"),
        })
    }
}

/// A full hex object id (SHA-1 or SHA-256).
pub fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A bundle file as its producer described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleFile {
    pub size: u64,
    pub sha256: String,
}

impl BundleFile {
    fn from_report(step: &'static str, report: &ScriptReport) -> Result<Self, TransferError> {
        let sha256 = report.require(step, "bundle_sha256")?;
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(TransferError::Protocol {
                step,
                message: format!("bundle_sha256 is not a sha256: {sha256}"),
            });
        }
        Ok(Self {
            size: report.require_u64(step, "bundle_size")?,
            sha256: sha256.to_ascii_lowercase(),
        })
    }
}

/// One config variable: the last key component (lowercase, as git reports it)
/// and its value. A key set without a value reads as `true`, which is what git
/// takes it to mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    pub var: String,
    pub value: String,
}

/// A non-origin remote the branch uses, as the source had it configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    pub name: String,
    pub entries: Vec<ConfigEntry>,
}

/// What a bundle leaves behind, reported so the caller can warn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotWarning {
    /// The repository has submodules; their commits are not in the bundle.
    Submodules,
    /// Paths are tracked with Git LFS; LFS objects are not in the bundle.
    Lfs,
}

/// A source checkout, captured: what [`SNAPSHOT_SCRIPT`] reports.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The branch tip.
    pub head: String,
    pub head_tree: String,
    /// `I`: the index as a commit on `head`.
    pub index_commit: String,
    pub tree_i: String,
    /// `W`: the working tree as a commit on `I`, and the bundle's only ref.
    pub snapshot: String,
    pub tree_w: String,
    /// How many destination tips the bundle was cut against.
    pub excluded: u32,
    pub bundle: BundleFile,
    pub branch_config: Vec<ConfigEntry>,
    pub remotes: Vec<RemoteConfig>,
    pub warnings: Vec<SnapshotWarning>,
}

impl Snapshot {
    /// Whether anything is staged, unstaged or untracked.
    pub fn has_uncommitted_changes(&self) -> bool {
        self.tree_i != self.head_tree || self.tree_w != self.tree_i
    }

    pub fn parse(stdout: &str, branch: &str) -> Result<Self, TransferError> {
        const STEP: &str = "snapshot";
        let report = ScriptReport::parse(stdout);
        let branch_prefix = format!("branch.{branch}.");
        let branch_config = decode_config(STEP, report.get("branch_config").unwrap_or(""))?
            .into_iter()
            .filter_map(|(key, value)| {
                let var = key.strip_prefix(&branch_prefix)?;
                (!var.is_empty() && !var.contains('.')).then(|| ConfigEntry {
                    var: var.to_string(),
                    value,
                })
            })
            .collect();
        let mut remotes = Vec::<RemoteConfig>::new();
        for hex in report.all("remote_config") {
            for (key, value) in decode_config(STEP, hex)? {
                let Some((name, var)) = key
                    .strip_prefix("remote.")
                    .and_then(|rest| rest.rsplit_once('.'))
                else {
                    continue;
                };
                let entry = ConfigEntry {
                    var: var.to_string(),
                    value,
                };
                match remotes.iter_mut().find(|r| r.name == name) {
                    Some(remote) => remote.entries.push(entry),
                    None => remotes.push(RemoteConfig {
                        name: name.to_string(),
                        entries: vec![entry],
                    }),
                }
            }
        }
        let warnings = report
            .all("warning")
            .filter_map(|w| match w {
                "submodules" => Some(SnapshotWarning::Submodules),
                "lfs" => Some(SnapshotWarning::Lfs),
                _ => None,
            })
            .collect();
        Ok(Self {
            head: report.require_oid(STEP, "head")?,
            head_tree: report.require_oid(STEP, "head_tree")?,
            index_commit: report.require_oid(STEP, "index_commit")?,
            tree_i: report.require_oid(STEP, "tree_i")?,
            snapshot: report.require_oid(STEP, "snapshot")?,
            tree_w: report.require_oid(STEP, "tree_w")?,
            excluded: report.require_u64(STEP, "excluded")? as u32,
            bundle: BundleFile::from_report(STEP, &report)?,
            branch_config,
            remotes,
            warnings,
        })
    }
}

/// Decode [`SNAPSHOT_SCRIPT`]'s hex-encoded `git config -z --get-regexp`
/// output: NUL-terminated entries of `key\nvalue`, or a bare `key` for a
/// variable set without a value.
fn decode_config(step: &'static str, hex: &str) -> Result<Vec<(String, String)>, TransferError> {
    let bytes = decode_hex(hex).ok_or_else(|| TransferError::Protocol {
        step,
        message: "config is not hex".to_string(),
    })?;
    Ok(bytes
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            match entry.split_once('\n') {
                Some((key, value)) => (key.to_string(), value.to_string()),
                None => (entry.into_owned(), "true".to_string()),
            }
        })
        .collect())
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

/// What [`REBUNDLE_SCRIPT`] reports.
pub fn parse_rebundle(stdout: &str) -> Result<BundleFile, TransferError> {
    BundleFile::from_report("rebundle", &ScriptReport::parse(stdout))
}

/// What [`ORIGIN_TIPS_SCRIPT`] reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OriginTips {
    /// Commits the destination has, to exclude from the bundle.
    pub tips: Vec<String>,
    /// Names origin does not have (a branch never pushed, say).
    pub unfetched: Vec<String>,
}

impl OriginTips {
    pub fn parse(stdout: &str) -> Self {
        let report = ScriptReport::parse(stdout);
        let mut tips: Vec<String> = Vec::new();
        for tip in report.all("tip").filter(|t| is_object_id(t)) {
            if !tips.iter().any(|t| t == tip) {
                tips.push(tip.to_string());
            }
        }
        Self {
            tips,
            unfetched: report.all("unfetched").map(str::to_string).collect(),
        }
    }
}

/// What [`APPLY_SCRIPT`] reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Whether this run created the branch (`branch` mode), rather than
    /// reusing one already at `head`.
    pub created_branch: bool,
    /// Remotes the destination did not have and now does.
    pub added_remotes: Vec<String>,
    /// Non-fatal problems, such as an upstream that could not be fetched.
    pub warnings: Vec<String>,
}

impl ApplyReport {
    pub fn parse(stdout: &str, head: &str) -> Result<Self, TransferError> {
        let report = ScriptReport::parse(stdout);
        let applied = report.require("apply", "applied")?;
        if applied != head {
            return Err(TransferError::Protocol {
                step: "apply",
                message: format!("applied {applied}, expected {head}"),
            });
        }
        Ok(Self {
            created_branch: report.get("created_branch") == Some("1"),
            added_remotes: report.all("added_remote").map(str::to_string).collect(),
            warnings: report.all("warning").map(str::to_string).collect(),
        })
    }
}

/// Split a script's output at its `repo=` lines: what [`PREFLIGHT_SCRIPT`] and
/// [`VERIFY_SCRIPT`] print for each checkout, in order.
pub fn sections(stdout: &str) -> Vec<ScriptReport> {
    let mut sections = Vec::new();
    let mut current: Option<String> = None;
    for line in stdout.lines() {
        if line.starts_with("repo=") {
            if let Some(text) = current.take() {
                sections.push(ScriptReport::parse(&text));
            }
            current = Some(String::new());
        } else if let Some(text) = current.as_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    if let Some(text) = current {
        sections.push(ScriptReport::parse(&text));
    }
    sections
}

/// One checkout's part of what [`VERIFY_SCRIPT`] reports: `None` when it is
/// still what its snapshot captured, else what changed. `Err` with the
/// message when the script could not tell.
pub fn parse_verify(report: &ScriptReport) -> Result<Option<String>, String> {
    if let Some(what) = report.get("changed") {
        return Ok(Some(what.to_string()));
    }
    if let Some(message) = report.get("error") {
        return Err(message.trim().to_string());
    }
    match report.get("unchanged") {
        Some("1") => Ok(None),
        _ => Err("the check stopped without an answer".to_string()),
    }
}

// =============================================================================
// Arguments
// =============================================================================

/// Arguments for [`SNAPSHOT_SCRIPT`].
pub fn snapshot_args(
    repo_dir: &str,
    branch: &str,
    bundle_path: &str,
    transfer_id: &str,
    exclude_tips: &[String],
) -> Vec<String> {
    let mut args = vec![
        repo_dir.to_string(),
        branch.to_string(),
        bundle_path.to_string(),
        transfer_id.to_string(),
    ];
    args.extend(exclude_tips.iter().filter(|t| is_object_id(t)).cloned());
    args
}

/// Arguments for [`REBUNDLE_SCRIPT`].
pub fn rebundle_args(repo_dir: &str, bundle_path: &str, transfer_id: &str) -> Vec<String> {
    vec![
        repo_dir.to_string(),
        bundle_path.to_string(),
        transfer_id.to_string(),
    ]
}

/// Arguments for [`VERIFY_SCRIPT`].
pub fn verify_args(
    repo_dir: &str,
    branch: &str,
    snapshot: &Snapshot,
    scratch_index: &str,
) -> Vec<String> {
    [
        repo_dir,
        branch,
        &snapshot.head,
        &snapshot.tree_i,
        &snapshot.tree_w,
        scratch_index,
    ]
    .map(str::to_string)
    .to_vec()
}

/// Arguments for [`ORIGIN_TIPS_SCRIPT`].
pub fn origin_tips_args(repo_dir: &str, branches: &[&str]) -> Vec<String> {
    std::iter::once(repo_dir)
        .chain(branches.iter().copied())
        .map(str::to_string)
        .collect()
}

/// Arguments for [`PREFLIGHT_SCRIPT`].
pub fn preflight_args<'a>(checkouts: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    checkouts
        .into_iter()
        .flat_map(|(dir, branch)| [dir.to_string(), branch.to_string()])
        .collect()
}

/// Arguments for [`CLEANUP_SCRIPT`].
pub fn cleanup_args(repo_dir: &str, transfer_id: &str, scratch_dir: Option<&str>) -> Vec<String> {
    vec![
        repo_dir.to_string(),
        transfer_id.to_string(),
        scratch_dir.unwrap_or_default().to_string(),
    ]
}

/// How [`APPLY_SCRIPT`] runs; see its documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    Checkout,
    Branch,
    Finish,
}

impl ApplyMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Checkout => "checkout",
            Self::Branch => "branch",
            Self::Finish => "finish",
        }
    }
}

/// One run of [`APPLY_SCRIPT`].
#[derive(Debug, Clone, Copy)]
pub struct ApplyRequest<'a> {
    pub mode: ApplyMode,
    /// The checkout (`Checkout`, `Finish`) or repository (`Branch`).
    pub repo_dir: &'a str,
    /// The bundle at the destination. Unused by `Finish`.
    pub bundle_path: &'a str,
    pub bundle: &'a BundleFile,
    pub transfer_id: &'a str,
    /// The branch's name at the destination, which may differ from the
    /// source's when the name was taken.
    pub branch: &'a str,
    pub snapshot: &'a Snapshot,
    /// Restore the staged, unstaged and untracked state (`Checkout`, `Finish`).
    pub restore: bool,
}

impl ApplyRequest<'_> {
    /// Named arguments, `--`, then — unless finishing — the config records:
    /// `b <var> <value>` for each `branch.<b>.<var>`, and per non-origin
    /// remote `R <name>` followed by `r <var> <value>` for each of its
    /// variables.
    pub fn args(&self) -> Vec<String> {
        let named = [
            ("mode", self.mode.as_str()),
            ("dir", self.repo_dir),
            ("bundle", self.bundle_path),
            ("sha256", &self.bundle.sha256),
            ("tid", self.transfer_id),
            ("branch", self.branch),
            ("head", &self.snapshot.head),
            ("snapshot", &self.snapshot.snapshot),
            ("tree_i", &self.snapshot.tree_i),
            ("tree_w", &self.snapshot.tree_w),
            ("restore", if self.restore { "1" } else { "0" }),
        ];
        let mut args: Vec<String> = named.iter().map(|(k, v)| format!("{k}={v}")).collect();
        args.push("--".to_string());
        if self.mode == ApplyMode::Finish {
            return args;
        }
        for entry in &self.snapshot.branch_config {
            args.extend(["b".to_string(), entry.var.clone(), entry.value.clone()]);
        }
        for remote in &self.snapshot.remotes {
            args.extend(["R".to_string(), remote.name.clone()]);
            for entry in &remote.entries {
                args.extend(["r".to_string(), entry.var.clone(), entry.value.clone()]);
            }
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    const OID_A: &str = "1111111111111111111111111111111111111111";
    const OID_B: &str = "2222222222222222222222222222222222222222";
    const OID_C: &str = "3333333333333333333333333333333333333333";

    fn snapshot_stdout(branch_config: &[u8], remotes: &[&[u8]]) -> String {
        let mut out = format!(
            "head={OID_A}\nhead_tree={OID_B}\nindex_commit={OID_A}\ntree_i={OID_B}\n\
             snapshot={OID_A}\ntree_w={OID_C}\nexcluded=2\nbundle_size=1234\n\
             bundle_sha256={}\nbranch_config={}\n",
            "ab".repeat(32),
            hex(branch_config)
        );
        for remote in remotes {
            out.push_str(&format!("remote_config={}\n", hex(remote)));
        }
        out.push_str("warning=lfs\nwarning=submodules\n");
        out
    }

    #[test]
    fn parses_a_snapshot_with_multiline_values_and_dotted_names() {
        let stdout = snapshot_stdout(
            b"branch.feat.x.remote\nfork\0branch.feat.x.merge\nrefs/heads/feat.x\0\
              branch.feat.x.description\nline one\nline two\0branch.feat.x.rebase\0",
            &[b"remote.my.fork.url\nhttps://example.com/a.git\0\
                remote.my.fork.fetch\n+refs/heads/*:refs/remotes/my.fork/*\0\
                remote.my.fork.fetch\n+refs/tags/*:refs/tags/*\0"],
        );

        let snapshot = Snapshot::parse(&stdout, "feat.x").unwrap();

        assert_eq!(snapshot.head, OID_A);
        assert_eq!(snapshot.excluded, 2);
        assert_eq!(
            snapshot.bundle,
            BundleFile {
                size: 1234,
                sha256: "ab".repeat(32)
            }
        );
        let config: Vec<(&str, &str)> = snapshot
            .branch_config
            .iter()
            .map(|e| (e.var.as_str(), e.value.as_str()))
            .collect();
        assert_eq!(
            config,
            [
                ("remote", "fork"),
                ("merge", "refs/heads/feat.x"),
                ("description", "line one\nline two"),
                ("rebase", "true"),
            ]
        );
        assert_eq!(snapshot.remotes.len(), 1);
        assert_eq!(snapshot.remotes[0].name, "my.fork");
        assert_eq!(snapshot.remotes[0].entries.len(), 3);
        assert_eq!(
            snapshot.warnings,
            [SnapshotWarning::Lfs, SnapshotWarning::Submodules]
        );
        assert!(snapshot.has_uncommitted_changes());
    }

    #[test]
    fn ignores_config_for_a_branch_that_merely_shares_a_prefix() {
        let stdout = snapshot_stdout(
            b"branch.feat.remote\norigin\0branch.feat.x.merge\nrefs/heads/other\0",
            &[],
        );
        let snapshot = Snapshot::parse(&stdout, "feat").unwrap();
        assert_eq!(
            snapshot.branch_config,
            [ConfigEntry {
                var: "remote".into(),
                value: "origin".into()
            }]
        );
    }

    #[test]
    fn an_empty_config_report_means_no_config() {
        let snapshot = Snapshot::parse(&snapshot_stdout(b"", &[]), "feat").unwrap();
        assert!(snapshot.branch_config.is_empty());
        assert!(snapshot.remotes.is_empty());
    }

    #[test]
    fn rejects_a_snapshot_report_with_a_missing_or_malformed_field() {
        let stdout = snapshot_stdout(b"", &[]).replace(&format!("head={OID_A}\n"), "");
        assert!(matches!(
            Snapshot::parse(&stdout, "feat"),
            Err(TransferError::Protocol { .. })
        ));
        let stdout = snapshot_stdout(b"", &[]).replace("excluded=2", "excluded=two");
        assert!(Snapshot::parse(&stdout, "feat").is_err());
        let stdout = snapshot_stdout(b"", &[]).replace(&"ab".repeat(32), "abc");
        assert!(Snapshot::parse(&stdout, "feat").is_err());
    }

    #[test]
    fn parses_origin_tips_without_duplicates() {
        let tips = OriginTips::parse(&format!(
            "tip={OID_A}\nunfetched=feat\ntip={OID_A}\ntip=nope\n"
        ));
        assert_eq!(tips.tips, [OID_A]);
        assert_eq!(tips.unfetched, ["feat"]);
    }

    #[test]
    fn apply_report_must_name_the_expected_head() {
        let ok = ApplyReport::parse(
            &format!("added_remote=fork\nwarning=cannot fetch\napplied={OID_A}\n"),
            OID_A,
        )
        .unwrap();
        assert_eq!(ok.added_remotes, ["fork"]);
        assert_eq!(ok.warnings, ["cannot fetch"]);
        assert!(!ok.created_branch);
        let created =
            ApplyReport::parse(&format!("created_branch=1\napplied={OID_A}\n"), OID_A).unwrap();
        assert!(created.created_branch);
        assert!(ApplyReport::parse(&format!("applied={OID_B}\n"), OID_A).is_err());
        assert!(ApplyReport::parse("", OID_A).is_err());
    }

    #[test]
    fn a_verify_report_names_the_change_or_says_unchanged() {
        let reports = sections(
            "repo=0\nunchanged=1\nrepo=1\nchanged=files changed\n\
             repo=2\nerror=cannot enter /x\nrepo=3\n",
        );
        let results: Vec<_> = reports.iter().map(parse_verify).collect();
        assert_eq!(
            results,
            [
                Ok(None),
                Ok(Some("files changed".to_string())),
                Err("cannot enter /x".to_string()),
                // A check that died before saying either is not "unchanged".
                Err("the check stopped without an answer".to_string()),
            ]
        );
    }

    #[test]
    fn snapshot_args_drop_anything_that_is_not_an_object_id() {
        let args = snapshot_args(
            "/r",
            "b",
            "/t/x.bundle",
            "tid",
            &[OID_A.to_string(), "; rm -rf /".to_string(), String::new()],
        );
        assert_eq!(args, ["/r", "b", "/t/x.bundle", "tid", OID_A]);
    }

    #[test]
    fn apply_args_carry_config_records_unless_finishing() {
        let snapshot = Snapshot::parse(
            &snapshot_stdout(
                b"branch.b.remote\nfork\0",
                &[b"remote.fork.url\n/path with space\0"],
            ),
            "b",
        )
        .unwrap();
        let mut request = ApplyRequest {
            mode: ApplyMode::Checkout,
            repo_dir: "/r",
            bundle_path: "/t/x.bundle",
            bundle: &snapshot.bundle,
            transfer_id: "tid",
            branch: "b-2",
            snapshot: &snapshot,
            restore: true,
        };
        let args = request.args();
        let tail = &args[args.iter().position(|a| a == "--").unwrap() + 1..];
        assert_eq!(
            tail,
            [
                "b",
                "remote",
                "fork",
                "R",
                "fork",
                "r",
                "url",
                "/path with space"
            ]
        );
        assert!(args.contains(&"branch=b-2".to_string()));
        assert!(args.contains(&"restore=1".to_string()));

        request.mode = ApplyMode::Finish;
        assert_eq!(request.args().last().unwrap(), "--");
    }
}
