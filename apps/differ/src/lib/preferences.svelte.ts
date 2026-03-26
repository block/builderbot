/**
 * User Preferences Store for Differ
 *
 * Manages persistent user preferences (Tauri store-backed).
 * Handles syntax theme selection with adaptive UI theming.
 */

import { invoke } from '@tauri-apps/api/core';
import {
  setSyntaxTheme,
  getTheme,
  isLightTheme,
  initHighlighter,
  registerExternalThemes,
  hasTheme,
  getRegisteredThemes,
  type SyntaxThemeName,
} from '@builderbot/diff-viewer/utils';
import { load, type Store } from '@tauri-apps/plugin-store';
import { listCustomThemes } from './commands';
import { createAdaptiveTheme, themeToVarMap } from '../../../staged/src/lib/theme';

// Re-export for convenience
export { isLightTheme };

// =============================================================================
// Constants
// =============================================================================

const SYNTAX_THEME_STORE_KEY = 'syntax-theme';
const CODE_FONT_FAMILY_STORE_KEY = 'code-font-family';
const CODE_FONT_SIZE_STORE_KEY = 'code-font-size';
const UI_FONT_SIZE_STORE_KEY = 'ui-font-size';

const DEFAULT_SYNTAX_THEME: SyntaxThemeName = 'laserwave';
const DEFAULT_CODE_FONT_FAMILY = 'SF Mono';
const DEFAULT_CODE_FONT_SIZE = 13;
const DEFAULT_UI_FONT_SIZE = 13;

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
  codeFontFamily: DEFAULT_CODE_FONT_FAMILY as string,
  codeFontSize: DEFAULT_CODE_FONT_SIZE as number,
  uiFontSize: DEFAULT_UI_FONT_SIZE as number,
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

function fontFamilyCSS(name: string): string {
  return `"${name.replace(/["\\]/g, '\\$&')}", monospace`;
}

function applyFontPreferences() {
  const style = document.documentElement.style;
  style.setProperty('--font-mono', fontFamilyCSS(preferences.codeFontFamily));
  style.setProperty('--code-font-size', `${preferences.codeFontSize}px`);
}

function applyUiFontSize() {
  document.documentElement.style.setProperty('--size-base', `${preferences.uiFontSize}px`);
}

// =============================================================================
// Initialization
// =============================================================================

export async function initPreferences(): Promise<void> {
  await initStore();

  // Load and register custom themes before validating saved preferences
  try {
    const customThemes = await listCustomThemes();
    if (customThemes.length > 0) {
      const parsed = customThemes
        .map((t) => {
          try {
            const theme = JSON.parse(t.json);
            const name =
              typeof theme.name === 'string' && theme.name.trim() ? theme.name.trim() : t.name;
            if (hasTheme(name)) {
              console.warn(`Skipping custom theme "${name}": conflicts with existing theme`);
              return null;
            }
            return { name, theme: { ...theme, name } };
          } catch {
            console.warn(`Failed to parse custom theme: ${t.name}`);
            return null;
          }
        })
        .filter((t): t is NonNullable<typeof t> => t !== null);
      if (parsed.length > 0) {
        registerExternalThemes(parsed);
      }
    }
  } catch {
    // Custom themes directory may not exist yet — that's fine
  }

  const savedTheme = await getStoreValue<string>(SYNTAX_THEME_STORE_KEY);
  if (savedTheme && hasTheme(savedTheme)) {
    preferences.syntaxTheme = savedTheme;
  }

  const savedFontFamily = await getStoreValue<string>(CODE_FONT_FAMILY_STORE_KEY);
  if (typeof savedFontFamily === 'string' && savedFontFamily.trim()) {
    preferences.codeFontFamily = savedFontFamily.trim();
  }

  const savedCodeFontSize = await getStoreValue<number>(CODE_FONT_SIZE_STORE_KEY);
  if (typeof savedCodeFontSize === 'number' && Number.isFinite(savedCodeFontSize)) {
    preferences.codeFontSize = Math.max(10, Math.min(20, savedCodeFontSize));
  }

  const savedUiFontSize = await getStoreValue<number>(UI_FONT_SIZE_STORE_KEY);
  if (typeof savedUiFontSize === 'number' && Number.isFinite(savedUiFontSize)) {
    preferences.uiFontSize = Math.max(10, Math.min(18, savedUiFontSize));
  }

  await initHighlighter(preferences.syntaxTheme as SyntaxThemeName);
  applyAdaptiveTheme();
  applyFontPreferences();
  applyUiFontSize();

  preferences.loaded = true;
}

// =============================================================================
// Theme Actions
// =============================================================================

export function getAvailableSyntaxThemes(): ThemeEntry[] {
  return getRegisteredThemes().map(({ name }) => ({ name }));
}

export async function selectSyntaxTheme(name: string): Promise<void> {
  await setSyntaxTheme(name as SyntaxThemeName);
  preferences.syntaxTheme = name;
  await setStoreValue(SYNTAX_THEME_STORE_KEY, name);
  applyAdaptiveTheme();
}

// =============================================================================
// Font Actions
// =============================================================================

export function setCodeFontFamily(family: string): void {
  preferences.codeFontFamily = family;
  applyFontPreferences();
  void setStoreValue(CODE_FONT_FAMILY_STORE_KEY, family);
}

export function setCodeFontSize(size: number): void {
  preferences.codeFontSize = Math.max(10, Math.min(20, size));
  applyFontPreferences();
  void setStoreValue(CODE_FONT_SIZE_STORE_KEY, preferences.codeFontSize);
}

export function setUiFontSize(size: number): void {
  preferences.uiFontSize = Math.max(10, Math.min(18, size));
  applyUiFontSize();
  void setStoreValue(UI_FONT_SIZE_STORE_KEY, preferences.uiFontSize);
}
