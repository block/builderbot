/**
 * User Preferences Store
 *
 * Manages persistent user preferences (localStorage-backed).
 * Handles UI scaling and syntax theme selection.
 *
 * Rebuildable: This module owns all preference state. The rest of the app
 * imports the reactive state directly - no subscriptions needed.
 */

import {
  SYNTAX_THEMES,
  setSyntaxTheme,
  getTheme,
  type SyntaxThemeName,
} from '../services/highlighter';
import { createAdaptiveTheme, themeToCssVars } from '../theme';

// =============================================================================
// Constants
// =============================================================================

const SIZE_STEP = 1;
const SIZE_MIN = 10;
const SIZE_MAX = 24;
const SIZE_DEFAULT = 13;

const SIZE_STORAGE_KEY = 'staged-size-base';
const SYNTAX_THEME_STORAGE_KEY = 'staged-syntax-theme';
const DEFAULT_SYNTAX_THEME: SyntaxThemeName = 'laserwave';

// =============================================================================
// Reactive State
// =============================================================================

/**
 * Preferences state object.
 * Use this directly in components - it's reactive!
 */
export const preferences = $state({
  /** Current UI size base (px) */
  sizeBase: SIZE_DEFAULT,
  /** Current syntax theme name */
  syntaxTheme: DEFAULT_SYNTAX_THEME as SyntaxThemeName,
  /** Version counter for triggering re-renders on theme change */
  syntaxThemeVersion: 0,
});

// =============================================================================
// CSS Application (internal)
// =============================================================================

function applySize() {
  document.documentElement.style.setProperty('--size-base', `${preferences.sizeBase}px`);
}

function applyCssVars(cssVars: string) {
  cssVars.split('\n').forEach((line) => {
    const match = line.match(/^\s*(--[\w-]+):\s*(.+);?\s*$/);
    if (match) {
      document.documentElement.style.setProperty(match[1], match[2].replace(';', ''));
    }
  });
}

function applyAdaptiveTheme() {
  const themeInfo = getTheme();
  if (themeInfo) {
    const adaptiveTheme = createAdaptiveTheme(themeInfo.bg, themeInfo.fg, themeInfo.comment);
    const cssVars = themeToCssVars(adaptiveTheme);
    applyCssVars(cssVars);
  }
}

// =============================================================================
// Getters
// =============================================================================

/**
 * Get all available syntax themes.
 */
export function getAvailableSyntaxThemes(): readonly SyntaxThemeName[] {
  return SYNTAX_THEMES;
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
    localStorage.setItem(SIZE_STORAGE_KEY, String(preferences.sizeBase));
  }
}

/**
 * Decrease UI size by one step.
 */
export function decreaseSize(): void {
  if (preferences.sizeBase > SIZE_MIN) {
    preferences.sizeBase -= SIZE_STEP;
    applySize();
    localStorage.setItem(SIZE_STORAGE_KEY, String(preferences.sizeBase));
  }
}

/**
 * Reset UI size to default.
 */
export function resetSize(): void {
  preferences.sizeBase = SIZE_DEFAULT;
  applySize();
  localStorage.setItem(SIZE_STORAGE_KEY, String(preferences.sizeBase));
}

/**
 * Load saved size preference and apply it.
 */
export function loadSavedSize(): void {
  const saved = localStorage.getItem(SIZE_STORAGE_KEY);
  if (saved) {
    const parsed = parseInt(saved, 10);
    if (!isNaN(parsed) && parsed >= SIZE_MIN && parsed <= SIZE_MAX) {
      preferences.sizeBase = parsed;
    }
  }
  applySize();
}

// =============================================================================
// Syntax Theme Actions
// =============================================================================

/**
 * Select a syntax theme by name.
 */
export async function selectSyntaxTheme(name: SyntaxThemeName): Promise<void> {
  preferences.syntaxTheme = name;
  await setSyntaxTheme(name);
  localStorage.setItem(SYNTAX_THEME_STORAGE_KEY, name);
  preferences.syntaxThemeVersion++;
  applyAdaptiveTheme();
}

/**
 * Cycle to the next syntax theme.
 */
export async function cycleSyntaxTheme(): Promise<void> {
  const currentIndex = SYNTAX_THEMES.indexOf(preferences.syntaxTheme);
  const nextIndex = (currentIndex + 1) % SYNTAX_THEMES.length;
  await selectSyntaxTheme(SYNTAX_THEMES[nextIndex]);
}

/**
 * Load saved syntax theme and apply it.
 * Also initializes the adaptive chrome theme.
 */
export async function loadSavedSyntaxTheme(): Promise<void> {
  const saved = localStorage.getItem(SYNTAX_THEME_STORAGE_KEY);
  if (saved && SYNTAX_THEMES.includes(saved as SyntaxThemeName)) {
    preferences.syntaxTheme = saved as SyntaxThemeName;
  }
  await setSyntaxTheme(preferences.syntaxTheme);
  applyAdaptiveTheme();
}

// =============================================================================
// Keyboard Shortcuts
// =============================================================================

/**
 * Handle preference-related keyboard shortcuts.
 * Returns true if the event was handled.
 */
export function handlePreferenceKeydown(event: KeyboardEvent): boolean {
  // Cmd/Ctrl + Shift + = (plus) to increase size
  // Cmd/Ctrl + Shift + - (minus) to decrease size
  // Cmd/Ctrl + Shift + 0 to reset size
  if ((event.metaKey || event.ctrlKey) && event.shiftKey) {
    if (event.key === '=' || event.key === '+') {
      event.preventDefault();
      increaseSize();
      return true;
    } else if (event.key === '-' || event.key === '_') {
      event.preventDefault();
      decreaseSize();
      return true;
    } else if (event.key === '0') {
      event.preventDefault();
      resetSize();
      return true;
    }
  }

  // T to cycle syntax themes (when not in input)
  if (event.key === 't' && !event.metaKey && !event.ctrlKey && !event.shiftKey) {
    const target = event.target as HTMLElement;
    if (target.tagName !== 'INPUT' && target.tagName !== 'TEXTAREA') {
      event.preventDefault();
      cycleSyntaxTheme();
      return true;
    }
  }

  return false;
}
