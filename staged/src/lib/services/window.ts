/**
 * Window Service
 *
 * Provides window management functions for creating new windows and getting window info.
 */

import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

/**
 * Get the current window's label.
 */
export async function getWindowLabel(): Promise<string> {
  return invoke<string>('get_window_label');
}

/**
 * Get the initial repository path from CLI arguments.
 * Returns null if no valid path was provided.
 */
export async function getInitialPath(): Promise<string | null> {
  return invoke<string | null>('get_initial_path');
}

/**
 * Install the CLI command to /usr/local/bin.
 * Returns the install path on success.
 * Throws an error with a message on failure.
 */
export async function installCli(): Promise<string> {
  return invoke<string>('install_cli');
}

/**
 * Open a URL in the default browser.
 */
export async function openUrl(url: string): Promise<void> {
  return invoke<void>('open_url', { url });
}

/**
 * Get the current window instance.
 */
export { getCurrentWindow };
