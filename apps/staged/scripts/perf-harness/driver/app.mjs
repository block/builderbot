// The app under test: its process and the two ways the driver talks to it,
// the web API (`/api/invoke`) and the harness routes (`/perf/*`).

import { spawn } from 'node:child_process';
import { openSync } from 'node:fs';

/** Start the app with exactly `env`. Its output goes to `logFile`. */
export function launchApp({ binary, env, cwd, logFile }) {
  const log = openSync(logFile, 'a');
  return spawn(binary, [], { env, cwd, stdio: ['ignore', log, log] });
}

export function createClient({ port, token }) {
  const base = `http://127.0.0.1:${port}`;
  const perf = async (path, init = {}) => {
    const response = await fetch(`${base}${path}`, {
      ...init,
      headers: { 'x-perf-token': token, ...init.headers },
    });
    if (!response.ok)
      throw new Error(`${path}: ${response.status} ${(await response.text()).slice(0, 300)}`);
    return response;
  };
  return {
    base,
    token,
    /** A command through the web API, counted apart from the page's own calls. */
    async invoke(command, args = {}) {
      const response = await fetch(`${base}/api/invoke/${command}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json', 'x-perf-driver': '1' },
        body: JSON.stringify(args),
      });
      const text = await response.text();
      if (!response.ok) throw new Error(`${command}: ${response.status} ${text.slice(0, 400)}`);
      return text ? JSON.parse(text) : null;
    },
    metrics: async () => (await perf('/perf/metrics')).json(),
    resetMetrics: () => perf('/perf/metrics/reset', { method: 'POST' }),
    /** Run a script in the app's own webviews; returns the windows it ran in. */
    evalInWebviews: async (script) =>
      (await perf('/perf/eval', { method: 'POST', body: script })).json(),
    takeResults: async () => (await perf('/perf/results')).json(),
  };
}
