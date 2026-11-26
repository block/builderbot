// The app under test: its process and the two ways the driver talks to it,
// the web API (`/api/invoke`) and the harness routes (`/perf/*`).
//
// The app's harness port serves nothing without the run's token (see
// `perf_harness/server.rs`). The driver sends it as a header on every
// request. The page cannot be made to: its calls are the app's own code,
// and a WebSocket takes no header. So the page gets the token as a cookie
// (`tokenCookie`, `tokenCookieScript`), which its browser then sends by
// itself with the document, its assets, every `fetch` and the handshake of
// the event socket.

import { spawn } from 'node:child_process';
import { openSync } from 'node:fs';

/** Start the app with exactly `env`. Its output goes to `logFile`. */
export function launchApp({ binary, env, cwd, logFile }) {
  const log = openSync(logFile, 'a');
  return spawn(binary, [], { env, cwd, stdio: ['ignore', log, log] });
}

export const TOKEN_HEADER = 'x-perf-token';
export const TOKEN_COOKIE = 'staged_perf_token';

/**
 * The page's credential for headless Chromium, as the parameters of the
 * DevTools `Network.setCookie`: set from outside the page before it first
 * loads, for this run's origin only, in a browser profile that exists for
 * the run. `httpOnly` keeps it from every script, the page's own included;
 * `Strict` keeps it off requests that start on another site.
 */
export const tokenCookie = ({ base, token }) => ({
  name: TOKEN_COOKIE,
  value: token,
  url: base,
  httpOnly: true,
  sameSite: 'Strict',
});

/**
 * The same for the app's own webview, where the only way in is a script
 * run through the harness's eval route (itself behind the token): a
 * session cookie set by the document the port answered with, so never
 * written to disk and gone with the app.
 */
export const tokenCookieScript = ({ token }) =>
  `document.cookie = ${JSON.stringify(`${TOKEN_COOKIE}=${token}; path=/; SameSite=Strict`)};`;

/** No request waits for ever: an app that stops answering fails the run with a message. */
const PERF_TIMEOUT_MS = 30_000;
const INVOKE_TIMEOUT_MS = 120_000;

export function createClient({ port, token }) {
  const base = `http://127.0.0.1:${port}`;
  // Every request carries the token: the app refuses one without it.
  const request = (path, init, timeout) =>
    fetch(`${base}${path}`, {
      ...init,
      headers: { [TOKEN_HEADER]: token, ...init.headers },
      signal: AbortSignal.timeout(timeout),
    }).catch((error) => {
      throw new Error(
        error.name === 'TimeoutError'
          ? `${path}: no answer from the app in ${timeout / 1000}s`
          : `${path}: ${error.cause?.message ?? error.message}`
      );
    });
  const perf = async (path, init = {}) => {
    const response = await request(path, init, PERF_TIMEOUT_MS);
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
     * the store holds a newer one the row could show, when it was stored,
     * and how many such it held at `at` (epoch ms, when the rows were read):
     * `{ "<tag>": { newestSeq, newerSeq, newerStoredMs, newerCount, floor } | null }`,
     * from the app's memory. `null` from hooks that predate the route, and
     * no `newerCount` from hooks that predate the count.
     */
    outdated: (shown, at) =>
      perf(`/perf/outdated${at === undefined ? '' : `?at=${at}`}`, {
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
