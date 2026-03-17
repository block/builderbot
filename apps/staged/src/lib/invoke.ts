/**
 * Traced invoke wrapper for Tauri commands.
 *
 * Every IPC call to the Rust backend is logged at `info` level with the
 * command name and wall-clock duration so we can identify calls that block
 * the UI thread for too long.
 */

import { invoke as tauriInvoke, type InvokeArgs } from '@tauri-apps/api/core';
import { info } from '@tauri-apps/plugin-log';

const SLOW_THRESHOLD_MS = 100;

/**
 * Drop-in replacement for `@tauri-apps/api/core.invoke` that logs timing.
 *
 * - Every call logs `[invoke] <command> completed in <N>ms`.
 * - Calls exceeding {@link SLOW_THRESHOLD_MS} are tagged `[SLOW]`.
 * - Failures log the elapsed time together with the error.
 */
export async function invoke<T>(cmd: string, args?: InvokeArgs): Promise<T> {
  const start = performance.now();
  try {
    const result = await tauriInvoke<T>(cmd, args);
    const elapsed = performance.now() - start;
    const tag = elapsed >= SLOW_THRESHOLD_MS ? ' [SLOW]' : '';
    info(`[invoke]${tag} ${cmd} completed in ${elapsed.toFixed(1)}ms`);
    return result;
  } catch (error) {
    const elapsed = performance.now() - start;
    info(`[invoke] ${cmd} failed after ${elapsed.toFixed(1)}ms: ${error}`);
    throw error;
  }
}
