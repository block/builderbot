/**
 * User Preferences Store for Differ
 *
 * Manages persistent user preferences (Tauri store-backed).
 * Handles syntax theme selection with adaptive UI theming.
 */

import { invoke } from '@tauri-apps/api/core';
import {
  SYNTAX_THEMES,
  setSyntaxTheme,
  getTheme,
  isLightTheme,
  initHighlighter,
  type SyntaxThemeName,
} from '@builderbot/diff-viewer/utils';
import { load, type Store } from '@tauri-apps/plugin-store';
import { createAdaptiveTheme, themeToVarMap } from '../../../staged/src/lib/theme';

// Re-export for convenience
export { isLightTheme };

// =============================================================================
// Constants
// =============================================================================

const SYNTAX_THEME_STORE_KEY = 'syntax-theme';
const DEFAULT_SYNTAX_THEME: SyntaxThemeName = 'laserwave';

// =============================================================================
// Store
// =============================================================================

let store: Store | null = null;

async function initStore(): Promise<void> {
  if (store) return;
  const storePath = await invoke<string>('preferences_store_path');
  store = await load(storePath, {
    defaults: {},
    autoSave: true,
    overrideDefaults: true,
  });
}

async function getStoreValue<T>(key: string): Promise<T | undefined> {
  if (!store) return undefined;
  return store.get<T>(key);
}

async function setStoreValue<T>(key: string, value: T): Promise<void> {
  if (!store) return;
  await store.set(key, value);
}

// =============================================================================
// Reactive State
// =============================================================================

export interface ThemeEntry {
  name: string;
}

export const preferences = $state({
  syntaxTheme: DEFAULT_SYNTAX_THEME as string,
  loaded: false,
});

// =============================================================================
// CSS Application
// =============================================================================

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

export async function initPreferences(): Promise<void> {
  await initStore();

  const savedTheme = await getStoreValue<string>(SYNTAX_THEME_STORE_KEY);
  if (savedTheme && SYNTAX_THEMES.includes(savedTheme as SyntaxThemeName)) {
    preferences.syntaxTheme = savedTheme;
  }

  await initHighlighter(preferences.syntaxTheme as SyntaxThemeName);
  applyAdaptiveTheme();

  preferences.loaded = true;
}

// =============================================================================
// Theme Actions
// =============================================================================

export function getAvailableSyntaxThemes(): ThemeEntry[] {
  return SYNTAX_THEMES.map((name) => ({ name }));
}

export async function selectSyntaxTheme(name: string): Promise<void> {
  await setSyntaxTheme(name as SyntaxThemeName);
  preferences.syntaxTheme = name;
  await setStoreValue(SYNTAX_THEME_STORE_KEY, name);
  applyAdaptiveTheme();
}
