// Re-export types from shared package
export { type ReviewState } from '@builderbot/diff-viewer/state';

// Re-export with Staged's Tauri commands pre-bound
import * as commands from '../../api/commands';
import { createReviewState as _create } from '@builderbot/diff-viewer/state';

/**
 * Create a reactive review state instance, pre-bound to Staged's Tauri commands.
 *
 * When `reviewId` is provided (e.g. clicking a specific review in the timeline),
 * the state loads that exact review by ID instead of searching by triple.
 */
export function createReviewState(
  branchId: string,
  commitSha: string,
  scope: 'branch' | 'commit',
  reviewId?: string
) {
  return _create(commands, branchId, commitSha, scope, reviewId);
}
