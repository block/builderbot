/**
 * Shared utility functions.
 */

import type { Project, Branch } from '../types';

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
