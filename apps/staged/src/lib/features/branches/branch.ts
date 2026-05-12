import { invokeCommand, isTauri, writeClipboardText } from '../../transport';

/** An application that can open a directory */
export interface OpenerApp {
  id: string;
  name: string;
  icon: string | null;
}

// Cache for performance
let cachedOpeners: OpenerApp[] | null = null;

/**
 * Get available applications that can open directories.
 * Results are cached for the lifetime of the app.
 */
export async function getAvailableOpeners(): Promise<OpenerApp[]> {
  if (cachedOpeners !== null) return cachedOpeners;
  if (!isTauri) {
    cachedOpeners = [];
    return cachedOpeners;
  }

  cachedOpeners = await invokeCommand<OpenerApp[]>('get_available_openers');
  return cachedOpeners;
}

/**
 * Open a directory in a specific application.
 */
export async function openInApp(path: string, appId: string): Promise<void> {
  if (!isTauri) {
    throw new Error('open_in_app is not available in web mode');
  }

  return invokeCommand<void>('open_in_app', { path, appId });
}

/**
 * Copy a path to the clipboard.
 */
export async function copyPathToClipboard(path: string): Promise<void> {
  await writeClipboardText(path);
}
