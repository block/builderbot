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
 * Returns the "least complete" status:
 * - 'conflict' (merge conflicts)
 * - 'closed' (closed PRs)
 * - 'open' (open PRs - includes draft, changes requested, pending, etc.)
 * - 'merged' (merged PRs)
 * - null (no branches with PRs)
 *
 * Priority order (most concerning first):
 * 1. No PR exists (null)
 * 2. Conflicts
 * 3. Closed PRs
 * 4. Open PRs (any state)
 * 5. Merged PRs
 */
export function aggregateProjectPrStatus(
  branches: Branch[]
): 'merged' | 'open' | 'closed' | 'conflict' | null {
  if (branches.length === 0) return null;

  let hasNoPr = false;
  let hasConflict = false;
  let hasClosed = false;
  let hasOpen = false;
  let hasMerged = false;

  for (const branch of branches) {
    // No PR is the least complete state
    if (!branch.prNumber) {
      hasNoPr = true;
      continue;
    }

    const { prState, prMergeable } = branch;

    // Check for merge conflicts
    if (prMergeable === false) {
      hasConflict = true;
      continue;
    }

    // Check for closed PRs
    if (prState === 'CLOSED') {
      hasClosed = true;
      continue;
    }

    // Check for merged PRs
    if (prState === 'MERGED') {
      hasMerged = true;
      continue;
    }

    // All other PRs are considered open
    hasOpen = true;
  }

  // Return the most concerning status
  if (hasNoPr) return null;
  if (hasConflict) return 'conflict';
  if (hasClosed) return 'closed';
  if (hasOpen) return 'open';
  if (hasMerged) return 'merged';

  return null;
}
