/**
 * Shared utility functions.
 */

import type { Project, Branch } from '../types';
import type { SessionType } from '../stores/sessionRegistry.svelte';
import type { RunActionPhase } from '../stores/projectRunActions.svelte';

/** Display name for a project: repo basename + optional subpath. */
export function projectDisplayName(p: Project): string {
  return p.name;
}

/**
 * Aggregate PR status indicator across all branches in a project.
 * Returns the most concerning PR state:
 * - 'merged': All PRs are merged
 * - 'open': PRs are open (default state)
 * - 'closed': PRs are closed without merging
 * - 'checks_failing': PRs have failing CI checks
 * - 'conflict': PRs have merge conflicts
 * - null: No branches with PRs
 *
 * Priority order (most concerning first):
 * 1. No PR exists (null)
 * 2. Conflict (merge conflicts)
 * 3. Checks failing (CI failures)
 * 4. Closed (closed without merging)
 * 5. Open (open PRs)
 * 6. Merged (merged PRs)
 */
export function aggregateProjectPrStatus(
  branches: Branch[]
): 'merged' | 'open' | 'closed' | 'checks_failing' | 'conflict' | null {
  if (branches.length === 0) return null;

  let hasNoPr = false;
  let hasConflict = false;
  let hasChecksFailing = false;
  let hasClosed = false;
  let hasOpen = false;
  let hasMerged = false;

  for (const branch of branches) {
    // No PR is the least complete state
    if (!branch.prNumber) {
      hasNoPr = true;
      continue;
    }

    const { prState, prMergeable, prChecksStatus } = branch;

    // Check for merged state
    if (prState === 'MERGED') {
      hasMerged = true;
      continue;
    }

    // Check for conflict state
    if (prMergeable === false) {
      hasConflict = true;
      continue;
    }

    // Check for closed state
    if (prState === 'CLOSED') {
      hasClosed = true;
      continue;
    }

    // Check for failing CI checks on open PRs
    if (prChecksStatus === 'FAILURE') {
      hasChecksFailing = true;
      continue;
    }

    // Default to open for other cases
    hasOpen = true;
  }

  // Return the most concerning status
  if (hasNoPr) return null;
  if (hasConflict) return 'conflict';
  if (hasChecksFailing) return 'checks_failing';
  if (hasClosed) return 'closed';
  if (hasOpen) return 'open';
  if (hasMerged) return 'merged';

  return null;
}

/**
 * Human-readable label for a session type.
 * Returns the singular noun form (e.g. "commit", "note").
 */
function sessionTypeLabel(type: SessionType): string {
  switch (type) {
    case 'commit':
      return 'commit';
    case 'note':
      return 'note';
    case 'review':
      return 'review';
    case 'pr':
      return 'PR';
    case 'push':
      return 'push';
    case 'other':
      return 'task';
  }
}

/**
 * Pluralize a noun: adds "s" when count !== 1, with special handling for
 * irregular forms used in session labels.
 */
function pluralize(word: string, count: number): string {
  if (count === 1) return word;
  // "PR" → "PRs", everything else just gets an "s"
  return `${word}s`;
}

/**
 * Build a project subtitle that summarizes the repo count and any running
 * session activity.
 *
 * Examples:
 *   "1 repo"                          (idle)
 *   "2 repos"                         (idle)
 *   "1 repo · making a commit"
 *   "2 repos · making a commit and a note"
 *   "1 repo · making commits and notes"
 *   "2 repos · pushing changes"
 */
export function projectSubtitle(
  repoCount: number,
  sessionTypes: SessionType[],
  runActionPhase?: RunActionPhase
): string {
  const repoLabel = `${repoCount} ${pluralize('repo', repoCount)}`;
  const activity = projectActivity(sessionTypes, runActionPhase);
  if (!activity) return repoLabel;
  return `${repoLabel} · ${activity}`;
}

/**
 * Activity portion of the project subtitle (without repo count prefix).
 * Returns empty string when idle.
 *
 * Examples:
 *   ""                       (idle)
 *   "building"
 *   "making a commit"
 *   "making a commit and a note"
 *   "pushing changes"
 */
export function projectActivity(
  sessionTypes: SessionType[],
  runActionPhase?: RunActionPhase
): string {
  if (sessionTypes.length === 0 && runActionPhase === 'building') {
    return 'building';
  }

  if (sessionTypes.length === 0) {
    return '';
  }

  const counts = new Map<SessionType, number>();
  for (const type of sessionTypes) {
    counts.set(type, (counts.get(type) ?? 0) + 1);
  }

  // Special-case "push" to read more naturally: "pushing changes" instead of "making a push"
  if (counts.size === 1 && counts.has('push')) {
    return 'pushing changes';
  }

  const displayOrder: SessionType[] = ['commit', 'note', 'review', 'pr', 'push', 'other'];
  const parts: string[] = [];

  for (const type of displayOrder) {
    const count = counts.get(type);
    if (!count) continue;

    const label = sessionTypeLabel(type);
    if (count === 1) {
      parts.push(`a ${label}`);
    } else {
      parts.push(pluralize(label, count));
    }
  }

  let activity: string;
  if (parts.length === 1) {
    activity = parts[0];
  } else if (parts.length === 2) {
    activity = `${parts[0]} and ${parts[1]}`;
  } else {
    activity = `${parts.slice(0, -1).join(', ')}, and ${parts[parts.length - 1]}`;
  }

  return `making ${activity}`;
}
