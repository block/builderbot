/**
 * User Preferences Store
 *
 * Manages persistent user preferences (Tauri store-backed).
 * Handles UI scaling and syntax theme selection.
 *
 * Uses Tauri's store plugin instead of localStorage to ensure preferences
 * persist across dev server restarts (localStorage is origin-scoped and
 * breaks when the dev port changes).
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
import {
  initPersistentStore,
  getStoreValue,
  setStoreValue,
  deleteStoreValue,
} from '../services/persistentStore';

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

// Store keys (prefixed for clarity)
const SIZE_STORE_KEY = 'size-base';
const SYNTAX_THEME_STORE_KEY = 'syntax-theme';
const SIDEBAR_POSITION_STORE_KEY = 'sidebar-position';
const SIDEBAR_WIDTH_STORE_KEY = 'sidebar-width';
const KEYBOARD_BINDINGS_STORE_KEY = 'keyboard-bindings';
const FEATURES_STORE_KEY = 'features';
const AI_AGENT_STORE_KEY = 'ai-agent';
const RECENT_REPOS_STORE_KEY = 'recent-repos';
const WINDOW_TABS_STORE_KEY_PREFIX = 'window-tabs-';
const VIEW_MODE_STORE_KEY = 'view-mode';

const DEFAULT_SYNTAX_THEME: SyntaxThemeName = 'laserwave';
const DEFAULT_SIDEBAR_POSITION: SidebarPosition = 'right';

const SIDEBAR_WIDTH_DEFAULT = 260;
const SIDEBAR_WIDTH_MIN = 180;
const SIDEBAR_WIDTH_MAX = 600;

// Export sidebar width constraints for use in components
export { SIDEBAR_WIDTH_MIN, SIDEBAR_WIDTH_MAX };

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
 * Known feature flags with their default values.
 * Add new flags here as the app evolves.
 */
export const DEFAULT_FEATURES = {} as const;

export type FeatureFlag = keyof typeof DEFAULT_FEATURES;

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
  /** Sidebar width in pixels */
  sidebarWidth: SIDEBAR_WIDTH_DEFAULT,
  /** Feature flags for experimental/optional features */
  features: { ...DEFAULT_FEATURES } as Record<string, boolean>,
  /** Selected AI agent (null if not yet chosen) */
  aiAgent: null as string | null,
  /** Whether all preferences have been loaded from storage */
  loaded: false,
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
// Initialization
// =============================================================================

/**
 * Initialize the preferences system.
 * Must be called once at app startup before loading preferences.
 */
export async function initPreferences(): Promise<void> {
  await initPersistentStore();
}

/**
 * Load all preferences from storage.
 * Sets preferences.loaded = true when complete.
 * Returns { viewMode, hasAgent } for App.svelte to use.
 */
export async function loadAllPreferences(): Promise<{ viewMode: ViewMode; hasAgent: boolean }> {
  // Load all preferences in parallel
  await Promise.all([
    loadSavedSize(),
    loadSavedSidebarPosition(),
    loadSavedSidebarWidth(),
    loadSavedFeatures(),
    loadSavedSyntaxTheme(),
  ]);

  // Load view mode and AI agent (these return values we need)
  const viewMode = await loadSavedViewMode();
  const hasAgent = await loadSavedAiAgent();

  // Mark preferences as fully loaded
  preferences.loaded = true;

  return { viewMode, hasAgent };
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

/**
 * Load saved size preference and apply it.
 */
export async function loadSavedSize(): Promise<void> {
  const saved = await getStoreValue<number>(SIZE_STORE_KEY);
  if (saved !== undefined && saved >= SIZE_MIN && saved <= SIZE_MAX) {
    preferences.sizeBase = saved;
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
  await setStoreValue(SYNTAX_THEME_STORE_KEY, name);
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

  const saved = await getStoreValue<string>(SYNTAX_THEME_STORE_KEY);

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
  setStoreValue(SIDEBAR_POSITION_STORE_KEY, position);
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
export async function loadSavedSidebarPosition(): Promise<void> {
  const saved = await getStoreValue<string>(SIDEBAR_POSITION_STORE_KEY);
  if (saved === 'left' || saved === 'right') {
    preferences.sidebarPosition = saved;
  }
}

// =============================================================================
// Sidebar Width Actions
// =============================================================================

/**
 * Set sidebar width (clamped to min/max bounds).
 */
export function setSidebarWidth(width: number): void {
  const clamped = Math.max(SIDEBAR_WIDTH_MIN, Math.min(SIDEBAR_WIDTH_MAX, width));
  preferences.sidebarWidth = clamped;
  setStoreValue(SIDEBAR_WIDTH_STORE_KEY, clamped);
}

/**
 * Reset sidebar width to default.
 */
export function resetSidebarWidth(): void {
  preferences.sidebarWidth = SIDEBAR_WIDTH_DEFAULT;
  setStoreValue(SIDEBAR_WIDTH_STORE_KEY, SIDEBAR_WIDTH_DEFAULT);
}

/**
 * Load saved sidebar width.
 */
export async function loadSavedSidebarWidth(): Promise<void> {
  const saved = await getStoreValue<number>(SIDEBAR_WIDTH_STORE_KEY);
  if (saved !== undefined && saved >= SIDEBAR_WIDTH_MIN && saved <= SIDEBAR_WIDTH_MAX) {
    preferences.sidebarWidth = saved;
  }
}

// =============================================================================
// Keyboard Bindings Storage
// =============================================================================

/**
 * Get all custom keyboard bindings from store.
 */
export async function getCustomKeyboardBindings(): Promise<Record<string, KeyboardBinding>> {
  const saved = await getStoreValue<Record<string, KeyboardBinding>>(KEYBOARD_BINDINGS_STORE_KEY);
  return saved ?? {};
}

/**
 * Save a custom keyboard binding.
 */
export async function saveCustomKeyboardBinding(
  id: string,
  binding: KeyboardBinding
): Promise<void> {
  const bindings = await getCustomKeyboardBindings();
  bindings[id] = binding;
  await setStoreValue(KEYBOARD_BINDINGS_STORE_KEY, bindings);
}

/**
 * Remove a custom keyboard binding (revert to default).
 */
export async function removeCustomKeyboardBinding(id: string): Promise<void> {
  const bindings = await getCustomKeyboardBindings();
  delete bindings[id];
  await setStoreValue(KEYBOARD_BINDINGS_STORE_KEY, bindings);
}

/**
 * Reset all custom keyboard bindings.
 */
export async function resetAllKeyboardBindings(): Promise<void> {
  await deleteStoreValue(KEYBOARD_BINDINGS_STORE_KEY);
}

// =============================================================================
// Feature Flags
// =============================================================================

/**
 * Check if a feature flag is enabled.
 */
export function isFeatureEnabled(flag: string): boolean {
  return preferences.features[flag] ?? false;
}

/**
 * Enable or disable a feature flag.
 */
export function setFeatureFlag(flag: string, enabled: boolean): void {
  preferences.features[flag] = enabled;
  setStoreValue(FEATURES_STORE_KEY, preferences.features);
}

/**
 * Toggle a feature flag.
 */
export function toggleFeatureFlag(flag: string): void {
  setFeatureFlag(flag, !isFeatureEnabled(flag));
}

/**
 * Load saved feature flags from store.
 * Merges with defaults so new flags get their default values.
 */
export async function loadSavedFeatures(): Promise<void> {
  const saved = await getStoreValue<Record<string, boolean>>(FEATURES_STORE_KEY);
  if (saved) {
    // Merge: defaults first, then saved values override
    preferences.features = { ...DEFAULT_FEATURES, ...saved };
  }
}

/**
 * Reset all feature flags to defaults.
 */
export async function resetFeatureFlags(): Promise<void> {
  preferences.features = { ...DEFAULT_FEATURES };
  await deleteStoreValue(FEATURES_STORE_KEY);
}

// =============================================================================
// AI Agent Preference
// =============================================================================

/**
 * Set the preferred AI agent.
 */
export function setAiAgent(agentId: string): void {
  preferences.aiAgent = agentId;
  setStoreValue(AI_AGENT_STORE_KEY, agentId);
}

/**
 * Load saved AI agent preference.
 * Returns true if a preference was found.
 */
export async function loadSavedAiAgent(): Promise<boolean> {
  const saved = await getStoreValue<string>(AI_AGENT_STORE_KEY);
  if (saved) {
    preferences.aiAgent = saved;
    return true;
  }
  return false;
}

/**
 * Check if an AI agent has been selected.
 */
export function hasAiAgentSelected(): boolean {
  return preferences.aiAgent !== null;
}

// =============================================================================
// Recent Repos (exported for repoState.svelte.ts)
// =============================================================================

export interface RepoEntry {
  path: string;
  name: string;
}

/**
 * Load recent repos from store.
 */
export async function loadRecentReposFromStore(): Promise<RepoEntry[]> {
  const saved = await getStoreValue<RepoEntry[]>(RECENT_REPOS_STORE_KEY);
  return saved ?? [];
}

/**
 * Save recent repos to store.
 */
export async function saveRecentReposToStore(repos: RepoEntry[]): Promise<void> {
  await setStoreValue(RECENT_REPOS_STORE_KEY, repos);
}

// =============================================================================
// Window Tabs (exported for tabState.svelte.ts)
// =============================================================================

export interface StoredTabData {
  tabs: Array<{
    id: string;
    projectId: string;
    repoPath: string;
    repoName: string;
    subpath: string | null;
  }>;
  activeTabIndex: number;
}

/**
 * Load window tabs from store.
 */
export async function loadWindowTabsFromStore(windowLabel: string): Promise<StoredTabData | null> {
  const key = `${WINDOW_TABS_STORE_KEY_PREFIX}${windowLabel}`;
  const saved = await getStoreValue<StoredTabData>(key);
  return saved ?? null;
}

/**
 * Save window tabs to store.
 */
export async function saveWindowTabsToStore(
  windowLabel: string,
  data: StoredTabData
): Promise<void> {
  const key = `${WINDOW_TABS_STORE_KEY_PREFIX}${windowLabel}`;
  await setStoreValue(key, data);
}

// =============================================================================
// View Mode (branches vs diff viewer)
// =============================================================================

export type ViewMode = 'branches' | 'diff';

const DEFAULT_VIEW_MODE: ViewMode = 'diff';

/**
 * Save view mode preference.
 */
export function saveViewMode(mode: ViewMode): void {
  setStoreValue(VIEW_MODE_STORE_KEY, mode);
}

/**
 * Load saved view mode preference.
 * Returns the saved mode or the default.
 */
export async function loadSavedViewMode(): Promise<ViewMode> {
  const saved = await getStoreValue<string>(VIEW_MODE_STORE_KEY);
  if (saved === 'branches' || saved === 'diff') {
    return saved;
  }
  return DEFAULT_VIEW_MODE;
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
