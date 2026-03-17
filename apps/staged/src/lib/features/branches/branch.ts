import { invoke } from '../../invoke';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';

/** An application that can open a directory */
export interface OpenerApp {
  id: string;
  name: string;
}

// Cache for performance
let cachedOpeners: OpenerApp[] | null = null;

/**
 * Get available applications that can open directories.
 * Results are cached for the lifetime of the app.
 */
export async function getAvailableOpeners(): Promise<OpenerApp[]> {
  if (cachedOpeners !== null) return cachedOpeners;
  cachedOpeners = await invoke<OpenerApp[]>('get_available_openers');
  return cachedOpeners;
}

/**
 * Open a directory in a specific application.
 */
export async function openInApp(path: string, appId: string): Promise<void> {
  return invoke<void>('open_in_app', { path, appId });
}

/**
 * Copy a path to the clipboard.
 */
export async function copyPathToClipboard(path: string): Promise<void> {
  await writeText(path);
}
