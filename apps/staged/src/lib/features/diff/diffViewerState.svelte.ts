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
 * Timestamp of the most recent diff-open trigger (e.g. a "Diff" button click),
 * recorded by `markDiffOpenClick`. Lets `createDiffViewerState` log the
 * click→open gap — the otherwise-invisible window spent mounting DiffModal and
 * its statically-imported DiffViewer before any diff work begins.
 */
let lastOpenClickAt: number | null = null;

/** Stamp the moment a diff-open was triggered, just before showing DiffModal. */
export function markDiffOpenClick() {
  lastOpenClickAt = performance.now();
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

  const clickToOpen =
    lastOpenClickAt !== null
      ? ` clickToOpen=${Math.round(performance.now() - lastOpenClickAt)}ms`
      : '';
  lastOpenClickAt = null;
  console.info(
    `[diff] open: branchId=${branchId} scope=${scope} commitSha=${commitSha ?? '(unresolved)'}${clickToOpen}`
  );

  async function loadFiles(generation: number): Promise<void> {
    state.loading = true;
    state.error = null;

    const t0 = performance.now();
    console.info(
      `[diff] loadFiles start: branchId=${state.branchId} scope=${state.scope} commitSha=${state.commitSha ?? '(unresolved)'}`
    );
    try {
      const response = await commands.getDiffFiles(
        state.branchId,
        state.commitSha ?? undefined,
        state.scope
      );
      if (generation !== contextGeneration) {
        console.info(
          `[diff] loadFiles stale (took ${Math.round(performance.now() - t0)}ms) — ignoring`
        );
        return;
      }

      state.commitSha = response.commitSha;
      state.files = response.files;
      console.info(
        `[diff] loadFiles done in ${Math.round(performance.now() - t0)}ms: files=${response.files.length} commitSha=${response.commitSha}`
      );

      if (state.files.length > 0) {
        await selectFile(sharedFileSummaryPath(state.files[0]));
      }
    } catch (e) {
      if (generation !== contextGeneration) return;
      state.error = e instanceof Error ? e.message : String(e);
      state.files = [];
      console.warn(
        `[diff] loadFiles failed in ${Math.round(performance.now() - t0)}ms: ${state.error}`
      );
    } finally {
      if (generation === contextGeneration) {
        state.loading = false;
      }
    }
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
    if (cached) {
      console.info(`[diff] loadFileDiff cache hit: path=${path}`);
      return cached;
    }

    state.loadingFile = path;

    const t0 = performance.now();
    console.info(`[diff] loadFileDiff start: path=${path} scope=${state.scope}`);
    try {
      const diff = await commands.getFileDiff(state.branchId, state.commitSha, state.scope, path);
      const newCache = new Map(state.diffCache);
      newCache.set(path, diff);
      state.diffCache = newCache;
      console.info(
        `[diff] loadFileDiff done in ${Math.round(performance.now() - t0)}ms: path=${path}`
      );
      return diff;
    } catch (e) {
      console.error(
        `[diff] loadFileDiff failed in ${Math.round(performance.now() - t0)}ms: path=${path}:`,
        e
      );
      return null;
    } finally {
      state.loadingFile = null;
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
    console.info(`[diff] switchContext: scope=${newScope} commitSha=${newCommitSha ?? '(none)'}`);
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
