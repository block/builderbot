export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Poll `probe` until it returns something truthy; the error names what was
 * awaited and the last answer. `giveUp` is asked before every poll and ends
 * the wait at once with the reason it returns: for what makes waiting
 * longer pointless, such as the app having exited.
 */
export async function until(what, probe, { timeout = 60_000, every = 250, giveUp } = {}) {
  const end = Date.now() + timeout;
  let last;
  for (;;) {
    const reason = giveUp?.();
    if (reason) throw new Error(`gave up waiting for ${what}: ${reason}`);
    try {
      last = await probe();
      if (last) return last;
    } catch (error) {
      last = error;
    }
    if (Date.now() >= end) break;
    await sleep(every);
  }
  throw new Error(
    `timed out after ${timeout / 1000}s waiting for ${what}: ${last instanceof Error ? last.message : JSON.stringify(last)}`
  );
}

export const log = (...parts) =>
  console.error(`[perf-harness ${new Date().toISOString().slice(11, 19)}]`, ...parts);
