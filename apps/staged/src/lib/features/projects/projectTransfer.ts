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

/**
 * Whether the dialog asks about uncommitted changes. Besides the repos known
 * to have some, an `unknown` repo (a workspace the transfer resumes first)
 * may turn out to, and the user's answer must still apply to it.
 */
export function asksAboutUncommitted(preflight: ProjectTransferPreflight): boolean {
  return preflight.hasUncommittedChanges || preflight.repos.some(isUnknown);
}

/** Whether the dialog only knows "may have" uncommitted changes: none seen,
 *  but some repo can't be inspected until the transfer resumes it. */
export function uncommittedOnlyPossible(preflight: ProjectTransferPreflight): boolean {
  return !preflight.hasUncommittedChanges && preflight.repos.some(isUnknown);
}

function isUnknown(row: { state: ProjectTransferRepoState }): boolean {
  return row.state === 'unknown';
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
