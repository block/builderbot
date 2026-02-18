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
 * - 'error' (checks failing, conflicts, closed PRs)
 * - 'warning' (changes requested)
 * - 'pending' (checks pending)
 * - 'success' (merged, approved, checks passed)
 * - 'neutral' (draft, no PR)
 * - null (no branches with PRs)
 *
 * Priority order (least complete first):
 * 1. No PR exists (null)
 * 2. Error states (closed, failing checks, conflicts)
 * 3. Warning states (changes requested)
 * 4. Pending states (checks pending)
 * 5. Success states (merged, approved, checks passed)
 * 6. Neutral states (draft)
 */
export function aggregateProjectPrStatus(
  branches: Branch[]
): 'success' | 'warning' | 'error' | 'neutral' | 'pending' | null {
  if (branches.length === 0) return null;

  let hasNoPr = false;
  let hasError = false;
  let hasWarning = false;
  let hasPending = false;
  let hasSuccess = false;
  let hasNeutral = false;

  for (const branch of branches) {
    // No PR is the least complete state
    if (!branch.prNumber) {
      hasNoPr = true;
      continue;
    }

    const { prState, prChecksStatus, prReviewDecision, prMergeable, prDraft } = branch;

    // A merged PR is terminal success; mergeability is no longer relevant.
    if (prState === 'MERGED') {
      hasSuccess = true;
      continue;
    }

    // Check for error states
    if (prState === 'CLOSED' || prChecksStatus === 'FAILURE' || prMergeable === false) {
      hasError = true;
      continue;
    }

    // Check for draft (neutral)
    if (prDraft) {
      hasNeutral = true;
      continue;
    }

    // Check for warning states
    if (prReviewDecision === 'CHANGES_REQUESTED') {
      hasWarning = true;
      continue;
    }

    // Check for pending states
    if (prChecksStatus === 'PENDING') {
      hasPending = true;
      continue;
    }

    // Check for success states
    if (
      prChecksStatus === 'SUCCESS' ||
      (prReviewDecision === 'APPROVED' && (prMergeable === true || prMergeable === null))
    ) {
      hasSuccess = true;
      continue;
    }

    // Default to neutral for other cases
    hasNeutral = true;
  }

  // Return the least complete status
  if (hasNoPr) return null;
  if (hasError) return 'error';
  if (hasWarning) return 'warning';
  if (hasPending) return 'pending';
  if (hasSuccess) return 'success';
  if (hasNeutral) return 'neutral';

  return null;
}
