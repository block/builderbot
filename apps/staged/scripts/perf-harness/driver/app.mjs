// The app under test: its process and the two ways the driver talks to it,
// the web API (`/api/invoke`) and the harness routes (`/perf/*`).

import { spawn } from 'node:child_process';
import { openSync } from 'node:fs';

/** Start the app with exactly `env`. Its output goes to `logFile`. */
export function launchApp({ binary, env, cwd, logFile }) {
  const log = openSync(logFile, 'a');
  return spawn(binary, [], { env, cwd, stdio: ['ignore', log, log] });
}

/** No request waits for ever: an app that stops answering fails the run with a message. */
const PERF_TIMEOUT_MS = 30_000;
const INVOKE_TIMEOUT_MS = 120_000;

export function createClient({ port, token }) {
  const base = `http://127.0.0.1:${port}`;
  const request = (path, init, timeout) =>
    fetch(`${base}${path}`, { ...init, signal: AbortSignal.timeout(timeout) }).catch((error) => {
      throw new Error(
        error.name === 'TimeoutError'
          ? `${path}: no answer from the app in ${timeout / 1000}s`
          : `${path}: ${error.cause?.message ?? error.message}`
      );
    });
  const perf = async (path, init = {}) => {
    const response = await request(
      path,
      { ...init, headers: { 'x-perf-token': token, ...init.headers } },
      PERF_TIMEOUT_MS
    );
    if (!response.ok) {
      const error = new Error(
        `${path}: ${response.status} ${(await response.text()).slice(0, 300)}`
      );
      error.status = response.status;
      throw error;
    }
    return response;
  };
  return {
    base,
    token,
    /** A command through the web API, counted apart from the page's own calls. */
    async invoke(command, args = {}) {
      const response = await request(
        `/api/invoke/${command}`,
        {
          method: 'POST',
          headers: { 'content-type': 'application/json', 'x-perf-driver': '1' },
          body: JSON.stringify(args),
        },
        INVOKE_TIMEOUT_MS
      );
      const text = await response.text();
      if (!response.ok) throw new Error(`${command}: ${response.status} ${text.slice(0, 400)}`);
      return text ? JSON.parse(text) : null;
    },
    metrics: async () => (await perf('/perf/metrics')).json(),
    resetMetrics: () => perf('/perf/metrics/reset', { method: 'POST' }),
    /**
     * For the stamp each row shows (`{ "<tag>": seq }`, 0 for none), whether
     * the store holds a newer one the row could show and when it was stored:
     * `{ "<tag>": { newestSeq, newerSeq, newerStoredMs, floor } | null }`,
     * from the app's memory. `null` from hooks that predate the route.
     */
    outdated: (shown) =>
      perf('/perf/outdated', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(shown),
      }).then(
        (response) => response.json().catch(() => null),
        (error) => {
          if (error.status >= 400 && error.status < 500) return null;
          throw error;
        }
      ),
    /** Run a script in the app's own webviews; returns the windows it ran in. */
    evalInWebviews: async (script) =>
      (await perf('/perf/eval', { method: 'POST', body: script })).json(),
    takeResults: async () => (await perf('/perf/results')).json(),
  };
}
