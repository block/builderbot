import { getBloxEnv } from '../commands';

/** Cached value of the BLOX_ENV environment variable (process-level, never changes). */
export let bloxEnv = $state<string | null>(null);

let initialized = false;

export async function initBloxEnv(): Promise<void> {
  if (initialized) return;
  initialized = true;
  try {
    bloxEnv = await getBloxEnv();
  } catch (e) {
    console.warn('Failed to read BLOX_ENV:', e);
  }
}
