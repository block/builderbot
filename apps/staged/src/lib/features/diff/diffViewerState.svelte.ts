// Re-export helper from shared package
export { fileSummaryPath } from '@builderbot/diff-viewer/state';
export type { DiffViewerState as SharedDiffViewerState } from '@builderbot/diff-viewer/state';

import * as commands from '../../api/commands';
import type { DiffScope } from '../../commands';
import type { FileDiff, FileDiffSummary } from '../../types';
import { fileSummaryPath as sharedFileSummaryPath } from '@builderbot/diff-viewer/state';

export interface DiffViewerState {
  branchId: string;
  commitSha: string | null;
  scope: DiffScope;
  files: FileDiffSummary[];
  diffCache: Map<string, FileDiff>;
  selectedFile: string | null;
  loading: boolean;
  loadingFile: string | null;
  error: string | null;
}

/**
 * Create a reactive diff viewer state instance, pre-bound to Staged's Tauri commands.
 */
export function createDiffViewerState(branchId: string, scope: DiffScope, commitSha?: string) {
  const state: DiffViewerState = $state({
    branchId,
    commitSha: commitSha ?? null,
    scope,
    files: [],
    diffCache: new Map(),
    selectedFile: null,
    loading: true,
    loadingFile: null,
    error: null,
  });

  let selectionGeneration = 0;
  let contextGeneration = 0;

  async function loadFiles(generation: number): Promise<void> {
    state.loading = true;
    state.error = null;

    try {
      const { data: response, revalidating } = await commands.getDiffFiles(
        state.branchId,
        state.commitSha ?? undefined,
        state.scope
      );
      if (generation !== contextGeneration) return;

      applyDiffFilesResponse(response);
      if (state.files.length > 0) {
        await selectFile(sharedFileSummaryPath(state.files[0]));
      }
      if (generation === contextGeneration) {
        state.loading = false;
      }

      if (revalidating) {
        const fresh = await revalidating;
        if (generation !== contextGeneration) return;
        applyDiffFilesResponse(fresh);
      }
    } catch (e) {
      if (generation !== contextGeneration) return;
      state.error = e instanceof Error ? e.message : String(e);
      state.files = [];
    } finally {
      if (generation === contextGeneration) {
        state.loading = false;
      }
    }
  }

  function applyDiffFilesResponse(response: { commitSha: string; files: FileDiffSummary[] }) {
    state.commitSha = response.commitSha;
    state.files = response.files;
  }

  async function selectFile(path: string | null): Promise<void> {
    const thisGeneration = ++selectionGeneration;
    state.selectedFile = path;

    if (path && !state.diffCache.has(path)) {
      await loadFileDiff(path);
      if (selectionGeneration !== thisGeneration) return;
    }
  }

  async function loadFileDiff(path: string): Promise<FileDiff | null> {
    if (!state.commitSha) return null;

    const cached = state.diffCache.get(path);
    if (cached) return cached;

    state.loadingFile = path;
    const commitSha = state.commitSha;
    const generation = contextGeneration;

    try {
      const { data: diff, revalidating } = await commands.getFileDiff(
        state.branchId,
        commitSha,
        state.scope,
        path
      );
      if (generation !== contextGeneration) return null;
      const newCache = new Map(state.diffCache);
      newCache.set(path, diff);
      state.diffCache = newCache;

      if (revalidating) {
        revalidating
          .then((fresh) => {
            if (generation !== contextGeneration) return;
            const next = new Map(state.diffCache);
            next.set(path, fresh);
            state.diffCache = next;
          })
          .catch(() => {});
      }
      return diff;
    } catch (e) {
      if (generation !== contextGeneration) return null;
      console.error(`Failed to load diff for ${path}:`, e);
      return null;
    } finally {
      if (generation === contextGeneration) {
        state.loadingFile = null;
      }
    }
  }

  function getCurrentDiff(): FileDiff | null {
    if (!state.selectedFile) return null;
    return state.diffCache.get(state.selectedFile) ?? null;
  }

  async function switchContext(
    newScope: 'branch' | 'commit',
    newCommitSha?: string
  ): Promise<void> {
    const generation = ++contextGeneration;
    state.scope = newScope;
    state.commitSha = newCommitSha ?? null;
    state.diffCache = new Map();
    state.selectedFile = null;
    state.loadingFile = null;
    state.files = [];
    state.error = null;
    await loadFiles(generation);
  }

  loadFiles(contextGeneration);

  return {
    state,
    selectFile,
    loadFileDiff,
    getCurrentDiff,
    switchContext,
  };
}
