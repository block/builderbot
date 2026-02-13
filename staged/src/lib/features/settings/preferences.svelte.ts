/**
 * User Preferences Store
 *
 * Manages persistent user preferences (Tauri store-backed).
 * Currently handles UI scaling and syntax theme selection.
 *
 * Uses Tauri's store plugin instead of localStorage to ensure preferences
 * persist across dev server restarts (localStorage is origin-scoped and
 * breaks when the dev port changes).
 */

import {
  SYNTAX_THEMES,
  setSyntaxTheme,
  getTheme,
  isLightTheme,
  type SyntaxThemeName,
} from '../diff/highlighter';
import { initPersistentStore, getStoreValue, setStoreValue } from '../../shared/persistentStore';
import { createAdaptiveTheme, themeToVarMap } from '../../theme';

// Re-export for convenience
export { isLightTheme };

// =============================================================================
// Constants
// =============================================================================

const SIZE_STEP = 1;
const SIZE_MIN = 10;
const SIZE_MAX = 24;
const SIZE_DEFAULT = 13;

const SIZE_STORE_KEY = 'size-base';
const SYNTAX_THEME_STORE_KEY = 'syntax-theme';
const RECENT_AGENTS_STORE_KEY = 'recent-agents';
/** Maximum number of recent agents to remember. */
const RECENT_AGENTS_MAX = 10;
const SIDEBAR_OPEN_STORE_KEY = 'sidebar-open';
const SIDEBAR_GROUP_BY_PROJECT_STORE_KEY = 'sidebar-group-by-project';
const SIDEBAR_WIDTH_STORE_KEY = 'sidebar-width';
const SIDEBAR_WIDTH_DEFAULT = 220;
const SIDEBAR_WIDTH_MIN = 140;
const SIDEBAR_WIDTH_MAX = 480;

const DEFAULT_SYNTAX_THEME: SyntaxThemeName = 'laserwave';

// =============================================================================
// Reactive State
// =============================================================================

/**
 * Theme entry for the theme picker.
 */
export interface ThemeEntry {
  name: string;
  isCustom: boolean;
}

/**
 * Preferences state object.
 * Use this directly in components - it's reactive!
 */
export const preferences = $state({
  /** Current UI size base (px) */
  sizeBase: SIZE_DEFAULT,
  /** Current syntax theme name */
  syntaxTheme: DEFAULT_SYNTAX_THEME as string,
  /**
   * Ordered list of recently used AI agent IDs, most-recent first.
   * Used to pick the best available agent for a given context (local vs remote).
   */
  recentAgents: [] as string[],
  /** Whether the left sidebar is visible */
  sidebarOpen: true,
  /** Whether sidebar branches are grouped by project */
  sidebarGroupByProject: true,
  /** Sidebar width in pixels */
  sidebarWidth: SIDEBAR_WIDTH_DEFAULT,
  /** Whether all preferences have been loaded from storage */
  loaded: false,
});

// =============================================================================
// CSS Application (internal)
// =============================================================================

function applySize() {
  document.documentElement.style.setProperty('--size-base', `${preferences.sizeBase}px`);
}

function applyAdaptiveTheme() {
  const themeInfo = getTheme();
  if (themeInfo) {
    const adaptiveTheme = createAdaptiveTheme(themeInfo.bg, themeInfo.fg, themeInfo.comment, {
      added: themeInfo.added,
      deleted: themeInfo.deleted,
      modified: themeInfo.modified,
    });
    const varMap = themeToVarMap(adaptiveTheme);
    const style = document.documentElement.style;
    for (const [prop, value] of Object.entries(varMap)) {
      style.setProperty(prop, value);
    }
  }
}

// =============================================================================
// Initialization
// =============================================================================

/**
 * Initialize preferences: load from store, apply theme, load Shiki.
 * Must be called once at app startup. Sets preferences.loaded = true when complete.
 */
export async function initPreferences(): Promise<void> {
  await initPersistentStore();

  // Load size
  const savedSize = await getStoreValue<number>(SIZE_STORE_KEY);
  if (savedSize !== undefined && savedSize >= SIZE_MIN && savedSize <= SIZE_MAX) {
    preferences.sizeBase = savedSize;
  }
  applySize();

  // Load syntax theme
  const savedTheme = await getStoreValue<string>(SYNTAX_THEME_STORE_KEY);
  if (savedTheme && SYNTAX_THEMES.includes(savedTheme as SyntaxThemeName)) {
    preferences.syntaxTheme = savedTheme;
  }
  await setSyntaxTheme(preferences.syntaxTheme as SyntaxThemeName);
  applyAdaptiveTheme();

  // Load recent agents list (with migration from legacy single-agent key)
  const savedRecent = await getStoreValue<string[]>(RECENT_AGENTS_STORE_KEY);
  if (savedRecent && Array.isArray(savedRecent) && savedRecent.length > 0) {
    preferences.recentAgents = savedRecent;
  } else {
    // Migrate from legacy single-agent preference
    const legacyAgent = await getStoreValue<string>('ai-agent');
    if (legacyAgent) {
      preferences.recentAgents = [legacyAgent];
      await setStoreValue(RECENT_AGENTS_STORE_KEY, [legacyAgent]);
    }
  }

  // Load sidebar preferences
  const savedSidebarOpen = await getStoreValue<boolean>(SIDEBAR_OPEN_STORE_KEY);
  if (savedSidebarOpen !== undefined) {
    preferences.sidebarOpen = savedSidebarOpen;
  }
  const savedSidebarGroup = await getStoreValue<boolean>(SIDEBAR_GROUP_BY_PROJECT_STORE_KEY);
  if (savedSidebarGroup !== undefined) {
    preferences.sidebarGroupByProject = savedSidebarGroup;
  }
  const savedSidebarWidth = await getStoreValue<number>(SIDEBAR_WIDTH_STORE_KEY);
  if (
    savedSidebarWidth !== undefined &&
    savedSidebarWidth >= SIDEBAR_WIDTH_MIN &&
    savedSidebarWidth <= SIDEBAR_WIDTH_MAX
  ) {
    preferences.sidebarWidth = savedSidebarWidth;
  }

  preferences.loaded = true;
}

// =============================================================================
// Theme Actions
// =============================================================================

/**
 * Get all available syntax themes, sorted alphabetically.
 */
export function getAvailableSyntaxThemes(): ThemeEntry[] {
  return SYNTAX_THEMES.map((name) => ({ name, isCustom: false }));
}

/**
 * Select a syntax theme by name.
 */
export async function selectSyntaxTheme(name: string): Promise<void> {
  await setSyntaxTheme(name as SyntaxThemeName);
  preferences.syntaxTheme = name;
  await setStoreValue(SYNTAX_THEME_STORE_KEY, name);
  applyAdaptiveTheme();
}

// =============================================================================
// Size Actions
// =============================================================================

/**
 * Increase UI size by one step.
 */
export function increaseSize(): void {
  if (preferences.sizeBase < SIZE_MAX) {
    preferences.sizeBase += SIZE_STEP;
    applySize();
    setStoreValue(SIZE_STORE_KEY, preferences.sizeBase);
  }
}

/**
 * Decrease UI size by one step.
 */
export function decreaseSize(): void {
  if (preferences.sizeBase > SIZE_MIN) {
    preferences.sizeBase -= SIZE_STEP;
    applySize();
    setStoreValue(SIZE_STORE_KEY, preferences.sizeBase);
  }
}

/**
 * Reset UI size to default.
 */
export function resetSize(): void {
  preferences.sizeBase = SIZE_DEFAULT;
  applySize();
  setStoreValue(SIZE_STORE_KEY, preferences.sizeBase);
}

// =============================================================================
// AI Agent Actions
// =============================================================================

/**
 * Record an agent as the most recently used.
 *
 * Moves `agentId` to the front of `recentAgents`, removing any prior
 * occurrence so the list stays deduplicated. The list is capped at
 * RECENT_AGENTS_MAX entries and persisted to disk.
 */
export function setAiAgent(agentId: string): void {
  const filtered = preferences.recentAgents.filter((id) => id !== agentId);
  preferences.recentAgents = [agentId, ...filtered].slice(0, RECENT_AGENTS_MAX);
  setStoreValue(RECENT_AGENTS_STORE_KEY, preferences.recentAgents);
}

/**
 * Return the most recently used agent that is present in `available`.
 *
 * Walks `recentAgents` in order and returns the first match, so local
 * and remote contexts each get the best agent for their environment.
 * Returns `null` if no recent agent is available.
 */
export function getPreferredAgent(available: { id: string }[]): string | null {
  const ids = new Set(available.map((a) => a.id));
  for (const agentId of preferences.recentAgents) {
    if (ids.has(agentId)) return agentId;
  }
  return null;
}

// =============================================================================
// Sidebar Actions
// =============================================================================

/** Toggle the sidebar open/closed and persist the choice. */
export function toggleSidebar(): void {
  preferences.sidebarOpen = !preferences.sidebarOpen;
  setStoreValue(SIDEBAR_OPEN_STORE_KEY, preferences.sidebarOpen);
}

/** Toggle the sidebar group-by-project mode and persist the choice. */
export function toggleSidebarGroupByProject(): void {
  preferences.sidebarGroupByProject = !preferences.sidebarGroupByProject;
  setStoreValue(SIDEBAR_GROUP_BY_PROJECT_STORE_KEY, preferences.sidebarGroupByProject);
}

/** Set the sidebar width (clamped) and persist the choice. */
export function setSidebarWidth(width: number): void {
  preferences.sidebarWidth = Math.round(
    Math.max(SIDEBAR_WIDTH_MIN, Math.min(SIDEBAR_WIDTH_MAX, width))
  );
  setStoreValue(SIDEBAR_WIDTH_STORE_KEY, preferences.sidebarWidth);
}
