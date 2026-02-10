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
} from '../services/highlighter';
import { initPersistentStore, getStoreValue, setStoreValue } from '../services/persistentStore';
import { createAdaptiveTheme, themeToVarMap } from '../theme';

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
