export const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Poll `probe` until it returns something truthy; the error names what was awaited and the last answer. */
export async function until(what, probe, { timeout = 60_000, every = 250 } = {}) {
  const end = Date.now() + timeout;
  let last;
  for (;;) {
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
