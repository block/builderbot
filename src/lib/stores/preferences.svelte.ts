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
  setCustomSyntaxTheme,
  getTheme,
  isLightTheme,
  isCustomTheme,
  registerCustomTheme,
  clearCustomThemes,
  getCustomThemeNames,
  type SyntaxThemeName,
} from '../services/highlighter';
import { getCustomThemes, readCustomTheme } from '../services/customThemes';

// Re-export for convenience
export { isLightTheme };
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
const SIDEBAR_POSITION_STORAGE_KEY = 'staged-sidebar-position';
const KEYBOARD_BINDINGS_STORAGE_KEY = 'staged-keyboard-bindings';
const DEFAULT_SYNTAX_THEME: SyntaxThemeName = 'laserwave';
const DEFAULT_SIDEBAR_POSITION: SidebarPosition = 'right';

export type SidebarPosition = 'left' | 'right';

/** Custom keyboard binding (keys + modifiers) */
export interface KeyboardBinding {
  keys: string[];
  modifiers?: {
    ctrl?: boolean;
    meta?: boolean;
    shift?: boolean;
    alt?: boolean;
  };
}

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
  /** Sidebar position (left or right) */
  sidebarPosition: DEFAULT_SIDEBAR_POSITION as SidebarPosition,
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
    const adaptiveTheme = createAdaptiveTheme(themeInfo.bg, themeInfo.fg, themeInfo.comment, {
      added: themeInfo.added,
      deleted: themeInfo.deleted,
      modified: themeInfo.modified,
    });
    const cssVars = themeToCssVars(adaptiveTheme);
    applyCssVars(cssVars);
  }
}

// =============================================================================
// Getters
// =============================================================================

/**
 * Theme entry for the theme picker (bundled or custom).
 */
export interface ThemeEntry {
  name: string;
  isCustom: boolean;
}

/**
 * Get all available syntax themes (bundled + custom), sorted alphabetically.
 */
export function getAvailableSyntaxThemes(): ThemeEntry[] {
  const bundled: ThemeEntry[] = SYNTAX_THEMES.map((name) => ({ name, isCustom: false }));
  const custom: ThemeEntry[] = getCustomThemeNames().map((name) => ({ name, isCustom: true }));

  // Merge and sort alphabetically
  return [...bundled, ...custom].sort((a, b) =>
    a.name.toLowerCase().localeCompare(b.name.toLowerCase())
  );
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
 * Select a syntax theme by name (bundled or custom).
 */
export async function selectSyntaxTheme(name: string): Promise<void> {
  // Check if it's a custom theme
  if (isCustomTheme(name)) {
    // Load custom theme JSON from backend
    const customThemes = await getCustomThemes();
    const theme = customThemes.find((t) => t.name === name);
    if (theme) {
      const themeJson = await readCustomTheme(theme.path);
      await setCustomSyntaxTheme(name, themeJson);
    }
  } else {
    // Bundled theme
    await setSyntaxTheme(name as SyntaxThemeName);
  }

  // Update state (cast to any since custom themes aren't in the type)
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  preferences.syntaxTheme = name as any;
  localStorage.setItem(SYNTAX_THEME_STORAGE_KEY, name);
  preferences.syntaxThemeVersion++;
  applyAdaptiveTheme();
}

/**
 * Cycle to the next syntax theme (bundled only for simplicity).
 */
export async function cycleSyntaxTheme(): Promise<void> {
  const currentIndex = SYNTAX_THEMES.indexOf(preferences.syntaxTheme as SyntaxThemeName);
  const nextIndex = currentIndex >= 0 ? (currentIndex + 1) % SYNTAX_THEMES.length : 0;
  await selectSyntaxTheme(SYNTAX_THEMES[nextIndex]);
}

/**
 * Discover and register custom themes from the backend.
 */
export async function loadCustomThemes(): Promise<void> {
  try {
    clearCustomThemes();
    const themes = await getCustomThemes();
    for (const theme of themes) {
      registerCustomTheme(theme.name, theme.is_light, theme.path);
    }
  } catch (e) {
    // Custom themes are optional - don't fail if backend unavailable
    console.warn('Failed to load custom themes:', e);
  }
}

/**
 * Load saved syntax theme and apply it.
 * Also initializes the adaptive chrome theme and discovers custom themes.
 */
export async function loadSavedSyntaxTheme(): Promise<void> {
  // First, discover custom themes
  await loadCustomThemes();

  const saved = localStorage.getItem(SYNTAX_THEME_STORAGE_KEY);

  if (saved) {
    // Check if it's a custom theme
    if (isCustomTheme(saved)) {
      try {
        await selectSyntaxTheme(saved);
        return;
      } catch {
        // Fall back to default if custom theme fails to load
        console.warn(`Failed to load custom theme "${saved}", using default`);
      }
    } else if (SYNTAX_THEMES.includes(saved as SyntaxThemeName)) {
      preferences.syntaxTheme = saved as SyntaxThemeName;
    }
  }

  await setSyntaxTheme(preferences.syntaxTheme);
  applyAdaptiveTheme();
}

// =============================================================================
// Sidebar Position Actions
// =============================================================================

/**
 * Set sidebar position.
 */
export function setSidebarPosition(position: SidebarPosition): void {
  preferences.sidebarPosition = position;
  localStorage.setItem(SIDEBAR_POSITION_STORAGE_KEY, position);
}

/**
 * Toggle sidebar position.
 */
export function toggleSidebarPosition(): void {
  setSidebarPosition(preferences.sidebarPosition === 'left' ? 'right' : 'left');
}

/**
 * Load saved sidebar position.
 */
export function loadSavedSidebarPosition(): void {
  const saved = localStorage.getItem(SIDEBAR_POSITION_STORAGE_KEY);
  if (saved === 'left' || saved === 'right') {
    preferences.sidebarPosition = saved;
  }
}

// =============================================================================
// Keyboard Bindings Storage
// =============================================================================

/**
 * Get all custom keyboard bindings from localStorage.
 */
export function getCustomKeyboardBindings(): Record<string, KeyboardBinding> {
  const saved = localStorage.getItem(KEYBOARD_BINDINGS_STORAGE_KEY);
  if (saved) {
    try {
      return JSON.parse(saved);
    } catch {
      return {};
    }
  }
  return {};
}

/**
 * Save a custom keyboard binding.
 */
export function saveCustomKeyboardBinding(id: string, binding: KeyboardBinding): void {
  const bindings = getCustomKeyboardBindings();
  bindings[id] = binding;
  localStorage.setItem(KEYBOARD_BINDINGS_STORAGE_KEY, JSON.stringify(bindings));
}

/**
 * Remove a custom keyboard binding (revert to default).
 */
export function removeCustomKeyboardBinding(id: string): void {
  const bindings = getCustomKeyboardBindings();
  delete bindings[id];
  localStorage.setItem(KEYBOARD_BINDINGS_STORAGE_KEY, JSON.stringify(bindings));
}

/**
 * Reset all custom keyboard bindings.
 */
export function resetAllKeyboardBindings(): void {
  localStorage.removeItem(KEYBOARD_BINDINGS_STORAGE_KEY);
}

// =============================================================================
// Keyboard Shortcuts
// =============================================================================

import { registerShortcuts, type Shortcut } from '../services/keyboard';

/**
 * Register preference-related keyboard shortcuts.
 * Returns a cleanup function.
 */
export function registerPreferenceShortcuts(): () => void {
  const shortcuts: Shortcut[] = [
    {
      id: 'pref-increase-size',
      keys: ['=', '+'],
      modifiers: { meta: true },
      description: 'Increase text size',
      category: 'view',
      handler: increaseSize,
    },
    {
      id: 'pref-decrease-size',
      keys: ['-'],
      modifiers: { meta: true },
      description: 'Decrease text size',
      category: 'view',
      handler: decreaseSize,
    },
    {
      id: 'pref-reset-size',
      keys: ['0'],
      modifiers: { meta: true },
      description: 'Reset text size',
      category: 'view',
      handler: resetSize,
    },
  ];

  return registerShortcuts(shortcuts);
}
