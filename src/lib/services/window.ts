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
 * Get the current window instance.
 */
export { getCurrentWindow };
