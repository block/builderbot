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
  loadAllThemePreviewColors,
  type SyntaxThemeName,
  type ThemePreviewColors,
} from '../diff/highlighter';
import { initPersistentStore, getStoreValue, setStoreValue } from '../../shared/persistentStore';
import { createAdaptiveTheme, themeToVarMap, type ThemeGitColors } from '../../theme';

// Re-export for convenience
export { isLightTheme, loadAllThemePreviewColors, type ThemePreviewColors };

// =============================================================================
// Constants
// =============================================================================

const SIZE_STEP = 1;
const SIZE_MIN = 10;
const SIZE_MAX = 24;
const SIZE_DEFAULT = 13;

const SIZE_STORE_KEY = 'size-base';
/** Legacy key: a single theme used to drive both chrome and diff. Migrated to `diff-theme`. */
const SYNTAX_THEME_STORE_KEY = 'syntax-theme';
/** The user-selectable theme that themes the diff viewer only. */
const DIFF_THEME_STORE_KEY = 'diff-theme';
/** The app chrome appearance: light / dark / system. */
const APP_MODE_STORE_KEY = 'app-mode';
const RECENT_AGENTS_STORE_KEY = 'recent-agents';
const AUTO_REVIEW_STORE_KEY = 'auto-start-code-reviews';
/** Maximum number of recent agents to remember. */
const RECENT_AGENTS_MAX = 10;

const DEFAULT_DIFF_THEME: SyntaxThemeName = 'laserwave';

/** App chrome appearance mode. `system` follows `prefers-color-scheme`. */
export type AppMode = 'light' | 'dark' | 'system';
const DEFAULT_APP_MODE: AppMode = 'system';

export type AutoReviewMode = 'never' | 'after-changes';

function normalizeSize(size: number): number {
  return Math.min(SIZE_MAX, Math.max(SIZE_MIN, Math.round(size)));
}

// =============================================================================
// Fixed chrome base colors
// =============================================================================

/**
 * Base colors fed into createAdaptiveTheme() for the app chrome. The chrome no
 * longer derives from the user's chosen diff/syntax theme — it has exactly one
 * fixed light and one fixed dark identity, selected by the resolved app mode.
 */
interface ModeBaseColors {
  bg: string;
  fg: string;
  comment: string;
  gitColors: ThemeGitColors;
}

/** Dark chrome — sourced from the previous laserwave-derived app.css :root defaults. */
const DARK_BASE: ModeBaseColors = {
  bg: '#27212e',
  fg: '#ffffff',
  comment: '#91889b',
  gitColors: { added: '#3fb950', deleted: '#f85149', modified: '#d29922' },
};

/** Light chrome — derived from a clean, legible light palette (github-light). */
const LIGHT_BASE: ModeBaseColors = {
  bg: '#ffffff',
  fg: '#24292e',
  comment: '#6e7781',
  gitColors: { added: '#28a745', deleted: '#d73a49', modified: '#2188ff' },
};

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
  /** App chrome appearance mode (light / dark / system). */
  mode: DEFAULT_APP_MODE as AppMode,
  /** Current diff-viewer theme name (Shiki theme, scoped to the diff only). */
  diffTheme: DEFAULT_DIFF_THEME as string,
  /**
   * CSS custom properties scoped to the diff-viewer container, derived from the
   * selected diff theme. Applied onto the diff wrapper element so the diff area
   * reflects the chosen theme while the surrounding chrome stays on `mode`.
   */
  diffThemeVars: {} as Record<string, string>,
  /** Bumped whenever the diff theme changes, to trigger diff re-highlighting. */
  diffThemeVersion: 0,
  /**
   * Ordered list of recently used AI agent IDs, most-recent first.
   * Used to pick the best available agent for a given context (local vs remote).
   */
  recentAgents: [] as string[],
  /** Whether auto code reviews are triggered after commits */
  autoReviewMode: 'after-changes' as AutoReviewMode,
  /** Whether all preferences have been loaded from storage */
  loaded: false,
});

// =============================================================================
// CSS Application (internal)
// =============================================================================

function applySize() {
  preferences.sizeBase = normalizeSize(preferences.sizeBase);
  document.documentElement.style.setProperty('--size-base', `${preferences.sizeBase}px`);
}

/** Resolve `system` to the OS preference; pass `light`/`dark` through. */
function resolveMode(mode: AppMode): 'light' | 'dark' {
  if (mode === 'system') {
    return typeof window !== 'undefined' &&
      window.matchMedia('(prefers-color-scheme: dark)').matches
      ? 'dark'
      : 'light';
  }
  return mode;
}

/**
 * Apply the app chrome theme onto the document root from the fixed base colors
 * selected by the resolved app mode. This drives every chrome CSS variable
 * (including `--theme-is-dark`, which the `darkMode` store observes) and is
 * independent of the user's diff theme.
 */
function applyChromeTheme() {
  const base = resolveMode(preferences.mode) === 'dark' ? DARK_BASE : LIGHT_BASE;
  const adaptiveTheme = createAdaptiveTheme(base.bg, base.fg, base.comment, base.gitColors);
  const varMap = themeToVarMap(adaptiveTheme);
  const style = document.documentElement.style;
  for (const [prop, value] of Object.entries(varMap)) {
    style.setProperty(prop, value);
  }
}

/**
 * Recompute the diff-viewer-scoped CSS variables from the active diff theme.
 * A second createAdaptiveTheme() run lets the diff area re-theme fully (bg,
 * text, tints) without touching the chrome — the resulting var map is applied
 * to the diff wrapper element by DiffModal. Also bumps the version so the diff
 * re-highlights against the new Shiki theme.
 */
function applyDiffTheme() {
  const themeInfo = getTheme();
  if (!themeInfo) return;
  const adaptiveTheme = createAdaptiveTheme(themeInfo.bg, themeInfo.fg, themeInfo.comment, {
    added: themeInfo.added,
    deleted: themeInfo.deleted,
    modified: themeInfo.modified,
  });
  preferences.diffThemeVars = themeToVarMap(adaptiveTheme);
  preferences.diffThemeVersion += 1;
}

let systemModeListenerAttached = false;
/** Re-apply chrome when the OS appearance changes, while in `system` mode. */
function ensureSystemModeListener() {
  if (systemModeListenerAttached || typeof window === 'undefined') return;
  systemModeListenerAttached = true;
  window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
    if (preferences.mode === 'system') {
      applyChromeTheme();
    }
  });
}

async function loadSqAvailabilityForDefault(): Promise<boolean> {
  // Keep preferences from taking a static dependency on the sq state module.
  const { ensureSqAvailabilityLoaded } = await import('./sq.svelte');
  return ensureSqAvailabilityLoaded();
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
  if (
    typeof savedSize === 'number' &&
    Number.isFinite(savedSize) &&
    savedSize >= SIZE_MIN &&
    savedSize <= SIZE_MAX
  ) {
    preferences.sizeBase = normalizeSize(savedSize);
  }
  applySize();

  // Load app chrome mode and apply the fixed light/dark chrome theme.
  const savedMode = await getStoreValue<AppMode>(APP_MODE_STORE_KEY);
  if (savedMode === 'light' || savedMode === 'dark' || savedMode === 'system') {
    preferences.mode = savedMode;
  }
  ensureSystemModeListener();
  applyChromeTheme();

  // Load diff theme (migrating from the legacy combined `syntax-theme` key).
  let savedDiffTheme = await getStoreValue<string>(DIFF_THEME_STORE_KEY);
  if (!savedDiffTheme) {
    savedDiffTheme = await getStoreValue<string>(SYNTAX_THEME_STORE_KEY);
  }
  if (savedDiffTheme && SYNTAX_THEMES.includes(savedDiffTheme as SyntaxThemeName)) {
    preferences.diffTheme = savedDiffTheme;
  }
  await setSyntaxTheme(preferences.diffTheme as SyntaxThemeName);
  applyDiffTheme();

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

  // Load auto-review mode
  const savedAutoReview = await getStoreValue<AutoReviewMode>(AUTO_REVIEW_STORE_KEY);
  if (savedAutoReview === 'never' || savedAutoReview === 'after-changes') {
    preferences.autoReviewMode = savedAutoReview;
  } else {
    preferences.autoReviewMode = (await loadSqAvailabilityForDefault()) ? 'after-changes' : 'never';
  }

  preferences.loaded = true;
}

// =============================================================================
// Theme Actions
// =============================================================================

/**
 * Get all available syntax themes (for the diff theme picker), sorted alphabetically.
 */
export function getAvailableSyntaxThemes(): ThemeEntry[] {
  return SYNTAX_THEMES.map((name) => ({ name, isCustom: false }));
}

/**
 * Set the app chrome appearance mode (light / dark / system).
 *
 * Only re-themes the chrome — the diff theme is untouched.
 */
export function setMode(mode: AppMode): void {
  preferences.mode = mode;
  setStoreValue(APP_MODE_STORE_KEY, mode);
  applyChromeTheme();
}

/**
 * Select a diff-viewer theme by name.
 *
 * Re-highlights the diff via the highlighter singleton and updates the
 * diff-scoped CSS variables. Deliberately does NOT touch the app chrome.
 */
export async function selectDiffTheme(name: string): Promise<void> {
  await setSyntaxTheme(name as SyntaxThemeName);
  preferences.diffTheme = name;
  await setStoreValue(DIFF_THEME_STORE_KEY, name);
  applyDiffTheme();
}

// =============================================================================
// Auto Review Actions
// =============================================================================

export function setAutoReviewMode(mode: AutoReviewMode): void {
  preferences.autoReviewMode = mode;
  setStoreValue(AUTO_REVIEW_STORE_KEY, mode);
}

// =============================================================================
// Size Actions
// =============================================================================

/**
 * Increase UI size by one step.
 */
export function increaseSize(): void {
  if (preferences.sizeBase < SIZE_MAX) {
    preferences.sizeBase = normalizeSize(preferences.sizeBase + SIZE_STEP);
    applySize();
    setStoreValue(SIZE_STORE_KEY, preferences.sizeBase);
  }
}

/**
 * Decrease UI size by one step.
 */
export function decreaseSize(): void {
  if (preferences.sizeBase > SIZE_MIN) {
    preferences.sizeBase = normalizeSize(preferences.sizeBase - SIZE_STEP);
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
 * Falls back to the first available agent when no recent match exists.
 * Returns `null` only when `available` is empty.
 */
export function getPreferredAgent(available: { id: string }[]): string | null {
  const ids = new Set(available.map((a) => a.id));
  for (const agentId of preferences.recentAgents) {
    if (ids.has(agentId)) return agentId;
  }
  return available[0]?.id ?? null;
}
