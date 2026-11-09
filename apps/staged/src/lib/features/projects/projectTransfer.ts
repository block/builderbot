/**
 * Copy and rules for moving a project between local worktrees and a Blox
 * workspace: the action's label, the confirmation dialog's per-repo summary
 * and issue copy, the progress banner's phase text, and which result warnings
 * the success toast surfaces. Pure, so the dialog, banner, project lists and
 * projectActions share one wording.
 */

import type {
  ProjectLocation,
  ProjectTransferDirection,
  ProjectTransferIssue,
  ProjectTransferPhase,
  ProjectTransferPreflight,
  ProjectTransferRepoPreflight,
  ProjectTransferRepoState,
  ProjectTransferResult,
} from '../../types';

/** Where a transfer from `location` goes. */
export function transferDirectionFrom(location: ProjectLocation): ProjectTransferDirection {
  return location === 'local' ? 'toRemote' : 'toLocal';
}

/** The menu item / dialog title: "Transfer to Blox" or "Transfer to local". */
export function transferActionLabel(direction: ProjectTransferDirection): string {
  return direction === 'toRemote' ? 'Transfer to Blox' : 'Transfer to local';
}

/** The destination as a sentence fragment ("to Blox", "to local"). */
export function transferDestinationLabel(direction: ProjectTransferDirection): string {
  return direction === 'toRemote' ? 'Blox' : 'local';
}

/** A transferring project's status tooltip in the project lists. */
export function transferringTitle(direction: ProjectTransferDirection | undefined): string {
  return direction ? `Transferring to ${transferDestinationLabel(direction)}…` : 'Transferring…';
}

/** Changes a transfer would carry (or leave in the backup) for one repo. */
export function uncommittedCount(row: ProjectTransferRepoPreflight): number {
  return row.staged + row.unstaged + row.untracked + row.conflicted;
}

/** The kinds of change behind `uncommittedCount`, e.g. "1 staged, 2 untracked". */
export function uncommittedBreakdown(row: ProjectTransferRepoPreflight): string | null {
  return (
    [
      row.staged > 0 && `${row.staged} staged`,
      row.unstaged > 0 && `${row.unstaged} unstaged`,
      row.untracked > 0 && `${row.untracked} untracked`,
      row.conflicted > 0 && `${row.conflicted} conflicted`,
    ]
      .filter(Boolean)
      .join(', ') || null
  );
}

/** The backend's warning on a repo whose ignored files are over the cap a
 *  transfer carries or backs up. */
export const IGNORED_TOO_LARGE = 'ignored_too_large';

function ignoredTooLarge(row: ProjectTransferRepoPreflight): boolean {
  return row.warnings.some((issue) => issue.code === IGNORED_TOO_LARGE);
}

/** Whether a transfer asked to carry ignored files moves this repo's. */
export function carriesIgnoredFiles(
  row: ProjectTransferRepoPreflight,
  includeIgnored: boolean
): boolean {
  return includeIgnored && row.ignoredFiles > 0 && !ignoredTooLarge(row);
}

/**
 * What happens to a repo's ignored files (outside ignored folders): "3
 * ignored files will be transferred" when `includeIgnored`, otherwise "…stay
 * behind (kept in the backup)", or just "…stay behind" over the cap, which
 * the repo's warning explains. Null with none.
 */
export function ignoredFilesSummary(
  row: ProjectTransferRepoPreflight,
  includeIgnored: boolean
): string | null {
  if (row.ignoredFiles === 0) return null;
  const files = plural(row.ignoredFiles, 'ignored file');
  if (carriesIgnoredFiles(row, includeIgnored)) return `${files} will be transferred`;
  const stay = `${files} ${row.ignoredFiles === 1 ? 'stays' : 'stay'} behind`;
  return ignoredTooLarge(row) ? stay : `${stay} (kept in the backup)`;
}

/** The ignored folders (`node_modules/`) a repo leaves behind, which are never
 *  carried or backed up, e.g. "2 ignored folders stay behind"; null with none. */
export function ignoredFoldersSummary(row: ProjectTransferRepoPreflight): string | null {
  if (row.ignoredDirs === 0) return null;
  return `${plural(row.ignoredDirs, 'ignored folder')} ${row.ignoredDirs === 1 ? 'stays' : 'stay'} behind`;
}

/**
 * The Git LFS objects a repo carries beside its bundle, e.g. "3 LFS objects
 * (12 MB) will be carried"; null with none. The ones the source lacks are a
 * warning of their own.
 */
export function lfsSummary(row: ProjectTransferRepoPreflight): string | null {
  if (row.lfsPresent === 0) return null;
  return `${plural(row.lfsPresent, 'LFS object')} (${formatBytes(row.lfsPresentBytes)}) will be carried`;
}

/**
 * Whether the dialog asks about uncommitted changes. Besides the repos known
 * to have some, an `unknown` repo (a workspace the transfer resumes first)
 * or one still being scanned may turn out to, and the user's answer must
 * still apply to it.
 */
export function asksAboutUncommitted(preflight: ProjectTransferPreflight): boolean {
  return preflight.hasUncommittedChanges || preflight.repos.some(notYetRead);
}

/** Whether the dialog only knows "may have" uncommitted changes: none seen,
 *  but some repo hasn't been read yet (its scan is still going, or it can't
 *  be inspected until the transfer resumes it). */
export function uncommittedOnlyPossible(preflight: ProjectTransferPreflight): boolean {
  return !preflight.hasUncommittedChanges && preflight.repos.some(notYetRead);
}

/** The label on a session or reply held back while its project transfers. */
export const QUEUED_FOR_TRANSFER_LABEL = 'Queued until the transfer completes';

/** The confirmation shown when Transfer is clicked before the scan is done. */
export const SCAN_UNFINISHED_CONFIRMATION =
  "The scan of what will be left behind hasn't finished; files not yet listed may be lost on the remote. Transfer anyway?";

/**
 * Whether the dialog asks about ignored files: only when some repo has some
 * outside ignored folders. A repo that can't be inspected until its workspace
 * resumes isn't asked about; its ignored files are backed up.
 */
export function asksAboutIgnored(preflight: ProjectTransferPreflight): boolean {
  return preflight.repos.some((row) => row.ignoredFiles > 0);
}

/**
 * Why the ignored-files checkbox can't be ticked, or null when it can: some
 * repo's ignored files are over the cap, and the backend refuses to start a
 * transfer asked to carry them. The repo's own warning gives the sizes.
 */
export function ignoredTransferBlocked(preflight: ProjectTransferPreflight): string | null {
  const over = preflight.repos.filter(ignoredTooLarge);
  if (over.length === 0) return null;
  const whose = over.length === 1 ? `${over[0].branchName}'s` : "Some branches'";
  return `${whose} ignored files are too large to transfer, so they can't move this time.`;
}

function notYetRead(row: { state: ProjectTransferRepoState; scanning: boolean }): boolean {
  return row.state === 'unknown' || row.scanning;
}

export function plural(count: number, singular: string, pluralForm = `${singular}s`): string {
  return `${count} ${count === 1 ? singular : pluralForm}`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/**
 * Friendlier copy for issue codes whose backend sentence states the problem
 * but not what to do about it. Everything else (sessions, workspace states,
 * operations in progress, LFS, renames…) already reads well and passes
 * through. Returning null keeps the backend's message.
 */
const ISSUE_COPY: Record<string, (issue: ProjectTransferIssue) => string | null> = {
  blox_not_authenticated: (issue) =>
    /not authenticated/i.test(issue.message)
      ? "You're not signed in to Blox. Run “sq login” in a terminal, then try again."
      : null,
  sq_unavailable: () =>
    "The sq CLI isn't installed, so Staged can't reach Blox. Install it, then try again.",
  detached_head: (issue) => `${issue.message} Check the branch out again before transferring.`,
  wrong_branch: (issue) => `${issue.message} Switch back to the project's branch first.`,
  not_a_checkout: () =>
    "The branch's checkout isn't a git repository any more, so there is nothing to carry.",
  conflicts: (issue) => `${issue.message} Resolve them, or abort the merge or rebase, first.`,
  mixed_branch_types: () =>
    "This branch isn't where the rest of the project is, so it can't move with it. Move it to another project first.",
};

/** The sentence to show for a blocker or warning. */
export function issueMessage(issue: ProjectTransferIssue): string {
  return ISSUE_COPY[issue.code]?.(issue) ?? issue.message;
}

/** Whether a Cancel can still stop the transfer: up to the database switch. */
export function isTransferCancellable(phase: ProjectTransferPhase): boolean {
  return (
    phase === 'checking' ||
    phase === 'preparing_destination' ||
    phase === 'snapshotting' ||
    phase === 'moving' ||
    phase === 'applying'
  );
}

/**
 * The progress banner's headline for `phase`. `repo` names the repo the
 * phase is about, when it is about one; `bytes` is the bundle being moved.
 */
export function transferPhaseLabel(
  phase: ProjectTransferPhase,
  direction: ProjectTransferDirection,
  repo: string | null = null,
  bytes: number | null = null
): string {
  const about = repo ? ` ${repo}` : '';
  switch (phase) {
    case 'checking':
      return 'Checking the project';
    case 'preparing_destination':
      return direction === 'toRemote' ? 'Preparing the Blox workspace' : 'Preparing local clones';
    case 'snapshotting':
      return `Snapshotting${about}`;
    case 'moving':
      return `Moving${about}${bytes !== null ? ` (${formatBytes(bytes)})` : ''}`;
    case 'applying':
      return `Applying${about}`;
    case 'committing':
      return 'Switching the project over';
    case 'tearing_down':
      return direction === 'toRemote' ? 'Removing the old worktrees' : 'Deleting the old workspace';
    case 'completed':
      return 'Transferred';
    case 'rolling_back':
      return 'Undoing the transfer';
    case 'failed':
      return 'Transfer failed';
  }
}

/**
 * Warnings the success toast should surface: whatever tearing the source down
 * couldn't do (`worktree_left`, `workspace_left`) and anything a repo picked
 * up during the transfer. A repo's warnings repeat its preflight's, which the
 * dialog already showed, so those are left out when `preflight` is known.
 */
export function transferResultWarnings(
  result: ProjectTransferResult,
  preflight: ProjectTransferPreflight | null = null
): ProjectTransferIssue[] {
  const seen = new Set(
    (preflight?.repos ?? []).flatMap((repo) =>
      repo.warnings.map((w) => `${repo.branchId}\0${w.code}\0${w.message}`)
    )
  );
  const repoWarnings = result.repos.flatMap((repo) =>
    repo.warnings.filter((w) => !seen.has(`${repo.branchId}\0${w.code}\0${w.message}`))
  );
  return [...result.warnings, ...repoWarnings];
}

/** Whether `message` is the backend's "the transfer was cancelled" failure. */
export function isTransferCancelledError(message: string): boolean {
  return /transfer was cancelled/i.test(message);
}

/**
 * Whether `message` is the backend refusing a transfer asked for in a
 * direction the project no longer goes ("The project is already on Blox; it
 * was transferred after this dialog opened, …"): another window moved it
 * while this one's dialog was open. Nothing was done.
 */
export function isTransferStaleError(message: string): boolean {
  return /was transferred after this dialog opened/i.test(message);
}
