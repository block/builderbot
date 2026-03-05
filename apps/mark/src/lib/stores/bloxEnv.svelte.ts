import { getBloxEnv } from '../commands';

/** Cached value of the BLOX_ENV environment variable (process-level, never changes). */
export const bloxEnv = $state<{ value: string | null }>({ value: null });

let initialized = false;

export async function initBloxEnv(): Promise<void> {
  if (initialized) return;
  initialized = true;
  try {
    bloxEnv.value = await getBloxEnv();
  } catch (e) {
    console.warn('Failed to read BLOX_ENV:', e);
  }
}
