// Re-export types and helpers from shared package
export { fileSummaryPath, type DiffViewerState } from '@builderbot/diff-viewer/state';

// Re-export with Mark's Tauri commands pre-bound
import * as commands from '../../commands';
import { createDiffViewerState as _create } from '@builderbot/diff-viewer/state';

/**
 * Create a reactive diff viewer state instance, pre-bound to Mark's Tauri commands.
 */
export function createDiffViewerState(
  branchId: string,
  scope: 'branch' | 'commit',
  commitSha?: string
) {
  return _create(commands, branchId, scope, commitSha);
}
