// Re-export types from shared package
export { type ReviewState, type ReferenceFile } from '@builderbot/diff-viewer/state';

// Re-export with Mark's Tauri commands pre-bound
import * as commands from '../../api/commands';
import { createReviewState as _create } from '@builderbot/diff-viewer/state';

/**
 * Create a reactive review state instance, pre-bound to Mark's Tauri commands.
 */
export function createReviewState(branchId: string, commitSha: string, scope: 'branch' | 'commit') {
  return _create(commands, branchId, commitSha, scope);
}
