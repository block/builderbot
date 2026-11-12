/**
 * The home page's view preferences: Grid or Board, which group the board is
 * by, and each board's collapsed and hidden columns.
 *
 * Frontend-only, loaded once from initPreferences and saved on change. They
 * deliberately don't follow other windows (no syncedStoreValue): switching
 * the view in one window leaves the others as they are. See boardPrefs.ts
 * for how stored ids are resolved and pruned.
 */

import { getStoreValue, setStoreValue } from '../../shared/persistentStore';
import type { TagCatalog } from '../tags/tagState';
import {
  normalizeBoardColumns,
  normalizeBoardGroupId,
  normalizeViewMode,
  withColumnFlag,
  type BoardColumnPrefs,
  type BoardColumnsPrefs,
  type ProjectsViewMode,
} from './boardPrefs';

const VIEW_MODE_STORE_KEY = 'projects-view-mode';
const BOARD_GROUP_STORE_KEY = 'projects-board-group';
const BOARD_COLUMNS_STORE_KEY = 'projects-board-columns';

function save(key: string, value: unknown): void {
  setStoreValue(key, value).catch((e) => {
    console.error(`[boardViewPrefs] Failed to save ${key}:`, e);
  });
}

export const boardViewPrefs = $state({
  viewMode: 'grid' as ProjectsViewMode,
  /** The chosen group id, unresolved: it may name a group since deleted. */
  groupId: null as string | null,
  columns: {} as BoardColumnsPrefs,
});

/** Read the stored preferences. Called once at startup, before the first paint. */
export async function loadBoardViewPrefs(): Promise<void> {
  const [viewMode, groupId, columns] = await Promise.all([
    getStoreValue<unknown>(VIEW_MODE_STORE_KEY),
    getStoreValue<unknown>(BOARD_GROUP_STORE_KEY),
    getStoreValue<unknown>(BOARD_COLUMNS_STORE_KEY),
  ]);
  boardViewPrefs.viewMode = normalizeViewMode(viewMode);
  boardViewPrefs.groupId = normalizeBoardGroupId(groupId);
  boardViewPrefs.columns = normalizeBoardColumns(columns);
}

export function setProjectsViewMode(mode: ProjectsViewMode): void {
  if (boardViewPrefs.viewMode === mode) return;
  boardViewPrefs.viewMode = mode;
  save(VIEW_MODE_STORE_KEY, mode);
}

export function setBoardGroup(groupId: string): void {
  if (boardViewPrefs.groupId === groupId) return;
  boardViewPrefs.groupId = groupId;
  save(BOARD_GROUP_STORE_KEY, groupId);
}

/**
 * Collapse or hide one of a board's columns, or undo it. The saved state is
 * pruned against `catalog`, so pass the loaded catalog only.
 */
export function setBoardColumnFlag(
  groupId: string,
  key: string,
  flag: keyof BoardColumnPrefs,
  on: boolean,
  catalog: Pick<TagCatalog, 'groupsById' | 'tagsById'>
): void {
  const columns = withColumnFlag(boardViewPrefs.columns, groupId, key, flag, on, catalog);
  boardViewPrefs.columns = columns;
  save(BOARD_COLUMNS_STORE_KEY, columns);
}
