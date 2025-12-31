/**
 * Syntax Highlighting Service
 *
 * Self-contained module that wraps Shiki for syntax highlighting.
 * All Shiki-specific logic lives here - the rest of the app just sees
 * simple Token[] arrays.
 *
 * Languages are lazy-loaded on demand for fast startup.
 *
 * Rebuildable: To swap highlighting libraries, rewrite this file
 * with the same exports. No other files need to change.
 */

import { createHighlighter, type Highlighter, type ThemedToken, type BundledLanguage } from 'shiki';

// Simple token type that doesn't leak Shiki internals
export interface Token {
  content: string;
  color: string;
}

// Theme info exposed to the app
export interface HighlighterTheme {
  name: string;
  bg: string;
  fg: string;
  comment: string; // Color used for comments - useful for muted UI text
}

// Singleton highlighter instance
let highlighter: Highlighter | null = null;
let currentTheme: HighlighterTheme | null = null;
let currentThemeName: string = 'github-dark';
let initPromise: Promise<void> | null = null;

// Available syntax themes (all Shiki bundled themes)
export const SYNTAX_THEMES = [
  // Dark themes
  'andromeeda',
  'aurora-x',
  'ayu-dark',
  'catppuccin-frappe',
  'catppuccin-macchiato',
  'catppuccin-mocha',
  'dark-plus',
  'dracula',
  'dracula-soft',
  'everforest-dark',
  'github-dark',
  'github-dark-default',
  'github-dark-dimmed',
  'github-dark-high-contrast',
  'gruvbox-dark-hard',
  'gruvbox-dark-medium',
  'gruvbox-dark-soft',
  'houston',
  'kanagawa-dragon',
  'kanagawa-wave',
  'laserwave',
  'material-theme',
  'material-theme-darker',
  'material-theme-ocean',
  'material-theme-palenight',
  'min-dark',
  'monokai',
  'night-owl',
  'nord',
  'one-dark-pro',
  'plastic',
  'poimandres',
  'red',
  'rose-pine',
  'rose-pine-moon',
  'slack-dark',
  'solarized-dark',
  'synthwave-84',
  'tokyo-night',
  'vesper',
  'vitesse-black',
  'vitesse-dark',
  // Light themes
  'catppuccin-latte',
  'everforest-light',
  'github-light',
  'github-light-default',
  'github-light-high-contrast',
  'gruvbox-light-hard',
  'gruvbox-light-medium',
  'gruvbox-light-soft',
  'kanagawa-lotus',
  'light-plus',
  'material-theme-lighter',
  'min-light',
  'one-light',
  'rose-pine-dawn',
  'slack-ochin',
  'snazzy-light',
  'solarized-light',
  'vitesse-light',
] as const;

export type SyntaxThemeName = (typeof SYNTAX_THEMES)[number];

// Static theme imports (Vite can't handle dynamic imports for these)
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const themeImports: Record<SyntaxThemeName, () => Promise<any>> = {
  // Dark themes
  andromeeda: () => import('shiki/themes/andromeeda.mjs'),
  'aurora-x': () => import('shiki/themes/aurora-x.mjs'),
  'ayu-dark': () => import('shiki/themes/ayu-dark.mjs'),
  'catppuccin-frappe': () => import('shiki/themes/catppuccin-frappe.mjs'),
  'catppuccin-macchiato': () => import('shiki/themes/catppuccin-macchiato.mjs'),
  'catppuccin-mocha': () => import('shiki/themes/catppuccin-mocha.mjs'),
  'dark-plus': () => import('shiki/themes/dark-plus.mjs'),
  dracula: () => import('shiki/themes/dracula.mjs'),
  'dracula-soft': () => import('shiki/themes/dracula-soft.mjs'),
  'everforest-dark': () => import('shiki/themes/everforest-dark.mjs'),
  'github-dark': () => import('shiki/themes/github-dark.mjs'),
  'github-dark-default': () => import('shiki/themes/github-dark-default.mjs'),
  'github-dark-dimmed': () => import('shiki/themes/github-dark-dimmed.mjs'),
  'github-dark-high-contrast': () => import('shiki/themes/github-dark-high-contrast.mjs'),
  'gruvbox-dark-hard': () => import('shiki/themes/gruvbox-dark-hard.mjs'),
  'gruvbox-dark-medium': () => import('shiki/themes/gruvbox-dark-medium.mjs'),
  'gruvbox-dark-soft': () => import('shiki/themes/gruvbox-dark-soft.mjs'),
  houston: () => import('shiki/themes/houston.mjs'),
  'kanagawa-dragon': () => import('shiki/themes/kanagawa-dragon.mjs'),
  'kanagawa-wave': () => import('shiki/themes/kanagawa-wave.mjs'),
  laserwave: () => import('shiki/themes/laserwave.mjs'),
  'material-theme': () => import('shiki/themes/material-theme.mjs'),
  'material-theme-darker': () => import('shiki/themes/material-theme-darker.mjs'),
  'material-theme-ocean': () => import('shiki/themes/material-theme-ocean.mjs'),
  'material-theme-palenight': () => import('shiki/themes/material-theme-palenight.mjs'),
  'min-dark': () => import('shiki/themes/min-dark.mjs'),
  monokai: () => import('shiki/themes/monokai.mjs'),
  'night-owl': () => import('shiki/themes/night-owl.mjs'),
  nord: () => import('shiki/themes/nord.mjs'),
  'one-dark-pro': () => import('shiki/themes/one-dark-pro.mjs'),
  plastic: () => import('shiki/themes/plastic.mjs'),
  poimandres: () => import('shiki/themes/poimandres.mjs'),
  red: () => import('shiki/themes/red.mjs'),
  'rose-pine': () => import('shiki/themes/rose-pine.mjs'),
  'rose-pine-moon': () => import('shiki/themes/rose-pine-moon.mjs'),
  'slack-dark': () => import('shiki/themes/slack-dark.mjs'),
  'solarized-dark': () => import('shiki/themes/solarized-dark.mjs'),
  'synthwave-84': () => import('shiki/themes/synthwave-84.mjs'),
  'tokyo-night': () => import('shiki/themes/tokyo-night.mjs'),
  vesper: () => import('shiki/themes/vesper.mjs'),
  'vitesse-black': () => import('shiki/themes/vitesse-black.mjs'),
  'vitesse-dark': () => import('shiki/themes/vitesse-dark.mjs'),
  // Light themes
  'catppuccin-latte': () => import('shiki/themes/catppuccin-latte.mjs'),
  'everforest-light': () => import('shiki/themes/everforest-light.mjs'),
  'github-light': () => import('shiki/themes/github-light.mjs'),
  'github-light-default': () => import('shiki/themes/github-light-default.mjs'),
  'github-light-high-contrast': () => import('shiki/themes/github-light-high-contrast.mjs'),
  'gruvbox-light-hard': () => import('shiki/themes/gruvbox-light-hard.mjs'),
  'gruvbox-light-medium': () => import('shiki/themes/gruvbox-light-medium.mjs'),
  'gruvbox-light-soft': () => import('shiki/themes/gruvbox-light-soft.mjs'),
  'kanagawa-lotus': () => import('shiki/themes/kanagawa-lotus.mjs'),
  'light-plus': () => import('shiki/themes/light-plus.mjs'),
  'material-theme-lighter': () => import('shiki/themes/material-theme-lighter.mjs'),
  'min-light': () => import('shiki/themes/min-light.mjs'),
  'one-light': () => import('shiki/themes/one-light.mjs'),
  'rose-pine-dawn': () => import('shiki/themes/rose-pine-dawn.mjs'),
  'slack-ochin': () => import('shiki/themes/slack-ochin.mjs'),
  'snazzy-light': () => import('shiki/themes/snazzy-light.mjs'),
  'solarized-light': () => import('shiki/themes/solarized-light.mjs'),
  'vitesse-light': () => import('shiki/themes/vitesse-light.mjs'),
};

// Theme change listeners
type ThemeChangeListener = (theme: HighlighterTheme) => void;
const themeChangeListeners: Set<ThemeChangeListener> = new Set();

export function onThemeChange(listener: ThemeChangeListener): () => void {
  themeChangeListeners.add(listener);
  return () => themeChangeListeners.delete(listener);
}

// Track which languages we've attempted to load (to avoid repeated failures)
const loadedLanguages = new Set<string>();
const failedLanguages = new Set<string>();

// Core languages loaded at startup (most common, fast init)
const CORE_LANGUAGES: BundledLanguage[] = [
  'typescript',
  'javascript',
  'json',
  'markdown',
  'html',
  'css',
];

// All supported languages (lazy loaded on demand)
const SUPPORTED_LANGUAGES: BundledLanguage[] = [
  // Core (loaded at startup)
  'typescript',
  'javascript',
  'json',
  'markdown',
  'html',
  'css',

  // Systems
  'rust',
  'go',
  'c',
  'cpp',
  'zig',
  'nim',

  // JVM/.NET
  'java',
  'kotlin',
  'scala',
  'groovy',
  'csharp',
  'fsharp',

  // Mobile
  'dart',
  'swift',
  'objective-c',

  // Scripting
  'python',
  'ruby',
  'php',
  'perl',
  'lua',

  // Functional
  'haskell',
  'elixir',
  'erlang',
  'clojure',
  'ocaml',

  // Data science
  'r',
  'julia',

  // Web frameworks
  'svelte',
  'vue',
  'astro',
  'scss',
  'sass',
  'less',

  // Shell
  'bash',
  'shellscript',
  'powershell',

  // Data formats
  'yaml',
  'toml',
  'xml',

  // Build systems
  'make',
  'cmake',
  'nix',

  // DevOps/config
  'dockerfile',
  'nginx',
  'graphql',
  'terraform',
  'prisma',

  // Blockchain
  'solidity',

  // Other
  'sql',
  'diff',
  'wasm',
  'latex',
];

// Map file extensions to Shiki language IDs
const EXTENSION_MAP: Record<string, BundledLanguage> = {
  // TypeScript/JavaScript
  ts: 'typescript',
  tsx: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',

  // Python
  py: 'python',
  pyi: 'python',
  pyw: 'python',

  // Rust
  rs: 'rust',

  // Go
  go: 'go',

  // Zig
  zig: 'zig',

  // Data formats
  json: 'json',
  jsonc: 'json',
  json5: 'json',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'toml',
  xml: 'xml',
  svg: 'xml',
  plist: 'xml',

  // Web
  html: 'html',
  htm: 'html',
  xhtml: 'html',
  css: 'css',
  scss: 'scss',
  sass: 'sass',
  less: 'less',
  svelte: 'svelte',
  vue: 'vue',
  astro: 'astro',

  // Shell
  sh: 'bash',
  bash: 'bash',
  zsh: 'bash',
  fish: 'bash',
  ksh: 'bash',
  ps1: 'powershell',
  psm1: 'powershell',

  // Docs
  md: 'markdown',
  markdown: 'markdown',
  mdx: 'markdown',

  // Database
  sql: 'sql',
  mysql: 'sql',
  pgsql: 'sql',

  // Diff
  diff: 'diff',
  patch: 'diff',

  // C family
  c: 'c',
  h: 'c',
  cpp: 'cpp',
  cc: 'cpp',
  cxx: 'cpp',
  hpp: 'cpp',
  hxx: 'cpp',
  hh: 'cpp',

  // JVM
  java: 'java',
  kt: 'kotlin',
  kts: 'kotlin',
  scala: 'scala',
  sc: 'scala',
  groovy: 'groovy',
  gradle: 'groovy',
  clj: 'clojure',
  cljs: 'clojure',
  cljc: 'clojure',

  // .NET
  cs: 'csharp',
  fs: 'fsharp',
  fsx: 'fsharp',
  fsi: 'fsharp',

  // Apple/Mobile
  swift: 'swift',
  m: 'objective-c',
  mm: 'objective-c',
  dart: 'dart',

  // Ruby
  rb: 'ruby',
  rake: 'ruby',
  gemspec: 'ruby',

  // PHP
  php: 'php',

  // Perl
  pl: 'perl',
  pm: 'perl',

  // Lua
  lua: 'lua',

  // Functional
  hs: 'haskell',
  lhs: 'haskell',
  ex: 'elixir',
  exs: 'elixir',
  erl: 'erlang',
  hrl: 'erlang',
  ml: 'ocaml',
  mli: 'ocaml',

  // Data science
  r: 'r',
  R: 'r',
  jl: 'julia',

  // Systems (additional)
  nim: 'nim',

  // Build systems
  makefile: 'make',
  mk: 'make',
  cmake: 'cmake',
  nix: 'nix',

  // DevOps
  dockerfile: 'dockerfile',
  tf: 'terraform',
  hcl: 'terraform',
  prisma: 'prisma',
  graphql: 'graphql',
  gql: 'graphql',
  nginx: 'nginx',

  // Blockchain
  sol: 'solidity',

  // Other
  wasm: 'wasm',
  wat: 'wasm',
  tex: 'latex',
  ltx: 'latex',
};

// Theme settings type from Shiki (not exported, so we define it here)
interface ThemeSetting {
  scope?: string | string[];
  settings?: { foreground?: string };
}

/**
 * Extract the comment color from a theme's token settings.
 * Falls back to the provided fallback color if not found.
 */
function extractCommentColor(settings: ThemeSetting[] | undefined, fallback: string): string {
  if (!settings) return fallback;

  for (const setting of settings) {
    if (!setting.scope || !setting.settings?.foreground) continue;

    const scopes = Array.isArray(setting.scope) ? setting.scope : [setting.scope];
    if (scopes.includes('comment')) {
      return setting.settings.foreground;
    }
  }

  return fallback;
}

/**
 * Initialize the highlighter with a theme.
 * Only loads core languages at startup for fast init.
 * Other languages are lazy-loaded on demand.
 *
 * This is idempotent - multiple calls return the same instance.
 */
export async function initHighlighter(themeName: string = 'github-dark'): Promise<void> {
  // Return existing instance if already initialized
  if (highlighter) {
    return;
  }

  // If initialization is in progress, wait for it
  if (initPromise) {
    return initPromise;
  }

  // Start initialization
  initPromise = (async () => {
    highlighter = await createHighlighter({
      themes: [themeName],
      langs: CORE_LANGUAGES,
    });

    // Mark core languages as loaded
    CORE_LANGUAGES.forEach((lang) => loadedLanguages.add(lang));

    // Set current theme name (used by highlightLines)
    currentThemeName = themeName;

    // Extract theme colors
    const theme = highlighter.getTheme(themeName);
    const fg = theme.fg || '#d4d4d4';
    currentTheme = {
      name: themeName,
      bg: theme.bg || '#1e1e1e',
      fg,
      comment: extractCommentColor(theme.settings as ThemeSetting[], fg),
    };
  })();

  return initPromise;
}

/**
 * Get the current theme info (background, foreground colors).
 * Returns null if highlighter not initialized.
 */
export function getTheme(): HighlighterTheme | null {
  return currentTheme;
}

/**
 * Detect language from file path/extension.
 * Returns null for unknown extensions.
 */
export function detectLanguage(filePath: string): BundledLanguage | null {
  // Handle special filenames
  const filename = filePath.split('/').pop()?.toLowerCase() || '';
  if (filename === 'dockerfile') return 'dockerfile';
  if (filename === 'makefile' || filename === 'gnumakefile') return 'make';
  if (filename === 'cmakelists.txt') return 'cmake';

  const ext = filePath.split('.').pop()?.toLowerCase() || '';
  return EXTENSION_MAP[ext] || null;
}

/**
 * Check if a language is in our supported set.
 */
function isSupportedLanguage(lang: string): lang is BundledLanguage {
  return SUPPORTED_LANGUAGES.includes(lang as BundledLanguage);
}

/**
 * Ensure a language is loaded, loading it lazily if needed.
 * Returns true if language is ready to use, false if unavailable.
 */
async function ensureLanguageLoaded(lang: BundledLanguage): Promise<boolean> {
  if (!highlighter) return false;

  // Already loaded
  if (loadedLanguages.has(lang)) return true;

  // Already failed to load
  if (failedLanguages.has(lang)) return false;

  // Not in our supported set
  if (!isSupportedLanguage(lang)) {
    failedLanguages.add(lang);
    return false;
  }

  // Try to load it
  try {
    await highlighter.loadLanguage(lang);
    loadedLanguages.add(lang);
    return true;
  } catch {
    failedLanguages.add(lang);
    return false;
  }
}

/**
 * Highlight a single line of code.
 * Returns tokens with content and color.
 *
 * If highlighter isn't ready or language unsupported, returns
 * a single token with the full content and default foreground color.
 */
export function highlightLine(code: string, lang: BundledLanguage | null): Token[] {
  const fallback = [{ content: code, color: currentTheme?.fg || '#d4d4d4' }];

  if (!highlighter || !currentTheme || !lang) {
    return fallback;
  }

  // If language isn't loaded yet, return fallback (will be loaded async)
  if (!loadedLanguages.has(lang)) {
    return fallback;
  }

  try {
    const result = highlighter.codeToTokens(code, {
      lang,
      theme: currentTheme.name,
    });

    const tokens = result.tokens[0] || [];
    return tokens.map((token: ThemedToken) => ({
      content: token.content,
      color: token.color || currentTheme!.fg,
    }));
  } catch {
    return fallback;
  }
}

/**
 * Prepare a language for highlighting (async).
 * Call this when a file is selected to ensure its language is loaded.
 * Returns true if language is ready.
 */
export async function prepareLanguage(filePath: string): Promise<boolean> {
  const lang = detectLanguage(filePath);
  if (!lang) return false;
  return ensureLanguageLoaded(lang);
}

/**
 * Highlight multiple lines at once (more efficient for full files).
 * Returns an array of token arrays, one per line.
 */
export function highlightLines(code: string, lang: BundledLanguage | null): Token[][] {
  const fallbackLine = (line: string) => [{ content: line, color: currentTheme?.fg || '#d4d4d4' }];

  if (!highlighter || !currentTheme || !lang || !loadedLanguages.has(lang)) {
    return code.split('\n').map(fallbackLine);
  }

  try {
    const result = highlighter.codeToTokens(code, {
      lang,
      theme: currentThemeName,
    });

    return result.tokens.map((lineTokens: ThemedToken[]) =>
      lineTokens.map((token: ThemedToken) => ({
        content: token.content,
        color: token.color || currentTheme!.fg,
      }))
    );
  } catch {
    return code.split('\n').map(fallbackLine);
  }
}

/**
 * Get the current syntax theme name.
 */
export function getSyntaxThemeName(): string {
  return currentThemeName;
}

/**
 * Switch to a different syntax theme.
 * Loads the theme if not already loaded, then updates currentTheme.
 */
export async function setSyntaxTheme(themeName: SyntaxThemeName): Promise<void> {
  if (!highlighter) {
    await initHighlighter(themeName);
    return;
  }

  // Load the theme if not already loaded
  const loadedThemes = highlighter.getLoadedThemes();
  if (!loadedThemes.includes(themeName)) {
    const themeImport = themeImports[themeName];
    if (themeImport) {
      await highlighter.loadTheme(themeImport());
    }
  }

  // Update current theme
  currentThemeName = themeName;
  const theme = highlighter.getTheme(themeName);
  const fg = theme.fg || '#d4d4d4';
  currentTheme = {
    name: themeName,
    bg: theme.bg || '#1e1e1e',
    fg,
    comment: extractCommentColor(theme.settings as ThemeSetting[], fg),
  };

  // Notify listeners
  themeChangeListeners.forEach((listener) => listener(currentTheme!));
}
