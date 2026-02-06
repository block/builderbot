/**
 * Custom Theme Service
 *
 * Handles loading custom VS Code themes from ~/.config/staged/themes/
 * Custom themes are discovered by the Rust backend and loaded into Shiki.
 */

import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';

/**
 * Metadata about a custom theme (from Rust backend).
 */
export interface CustomTheme {
  /** Theme name (from JSON or filename) */
  name: string;
  /** Whether this is a light theme */
  is_light: boolean;
  /** Full path to the theme file */
  path: string;
}

/**
 * Result of validating a theme file.
 */
export interface ThemeValidation {
  /** Whether the theme is valid */
  valid: boolean;
  /** Theme name (if valid) */
  name: string | null;
  /** Whether it's a light theme (if valid) */
  is_light: boolean | null;
  /** Error message (if invalid) */
  error: string | null;
}

/**
 * Get list of custom themes from ~/.config/staged/themes/
 */
export async function getCustomThemes(): Promise<CustomTheme[]> {
  return invoke<CustomTheme[]>('get_custom_themes');
}

/**
 * Read the full JSON content of a custom theme file.
 * Returns the raw JSON string for loading into Shiki.
 */
export async function readCustomTheme(path: string): Promise<string> {
  return invoke<string>('read_custom_theme', { path });
}

/**
 * Get the path to the themes directory (creates it if needed).
 * Useful for showing users where to put custom themes.
 */
export async function getThemesDir(): Promise<string> {
  return invoke<string>('get_themes_dir');
}

/**
 * Open the themes directory in the system file manager.
 */
export async function openThemesDir(): Promise<void> {
  return invoke<void>('open_themes_dir');
}

/**
 * Validate theme JSON content without installing.
 */
export async function validateTheme(content: string): Promise<ThemeValidation> {
  return invoke<ThemeValidation>('validate_theme', { content });
}

/**
 * Install a theme from JSON content.
 * Returns the installed theme metadata.
 */
export async function installTheme(content: string, filename: string): Promise<CustomTheme> {
  return invoke<CustomTheme>('install_theme', { content, filename });
}

/**
 * Open a file picker dialog for theme files.
 * Returns the file path if selected, null if cancelled.
 */
export async function pickThemeFile(): Promise<string | null> {
  const result = await open({
    multiple: false,
    filters: [{ name: 'VS Code Theme', extensions: ['json'] }],
  });
  return result as string | null;
}

/**
 * Read a JSON file from disk (for file picker).
 */
export async function readJsonFile(path: string): Promise<string> {
  return invoke<string>('read_json_file', { path });
}
