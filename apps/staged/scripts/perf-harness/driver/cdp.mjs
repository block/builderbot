// A minimal Chrome DevTools Protocol client, so the harness needs a Chromium
// executable and no npm dependency.

import { spawn } from 'node:child_process';
import { openSync } from 'node:fs';

/** How long a DevTools call may take. The page-side waits in `ui.mjs` are shorter. */
export const CALL_TIMEOUT_MS = 120_000;

/**
 * The calls and events of one DevTools socket, already open. A call that
 * gets no answer in `timeout` ms is rejected, and so is every call in
 * flight when the socket closes or fails and every call made after that:
 * a lost browser fails the run with a message, where it used to leave the
 * driver waiting for an answer that could not come.
 */
export function connection(socket, { timeout = CALL_TIMEOUT_MS } = {}) {
  let nextId = 0;
  let lost = null;
  const pending = new Map();
  const listeners = new Map();
  const settle = (id) => {
    const call = pending.get(id);
    if (!call) return null;
    pending.delete(id);
    clearTimeout(call.timer);
    return call;
  };
  const lose = (why) => {
    if (lost) return;
    lost = why;
    for (const id of [...pending.keys()]) {
      const call = settle(id);
      call.reject(new Error(`${call.method}: ${why}`));
    }
  };
  socket.addEventListener('close', (event) =>
    lose(`the DevTools connection closed${event?.code ? ` (code ${event.code})` : ''}`)
  );
  socket.addEventListener('error', () => lose('the DevTools connection failed'));
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    if (message.id !== undefined) {
      const call = settle(message.id);
      if (message.error) call?.reject(new Error(`${call.method}: ${message.error.message}`));
      else call?.resolve(message.result);
      return;
    }
    for (const handler of listeners.get(message.method) ?? [])
      handler(message.params, message.sessionId);
  });
  return {
    send: (method, params = {}, sessionId) =>
      new Promise((resolve, reject) => {
        if (lost) return reject(new Error(`${method}: ${lost}`));
        const id = ++nextId;
        const timer = setTimeout(() => {
          if (settle(id))
            reject(new Error(`${method}: no answer from the browser in ${timeout / 1000}s`));
        }, timeout);
        pending.set(id, { resolve, reject, method, timer });
        try {
          socket.send(JSON.stringify({ id, method, params, sessionId }));
        } catch (error) {
          settle(id);
          reject(new Error(`${method}: ${error.message}`));
        }
      }),
    on: (method, handler) => listeners.set(method, [...(listeners.get(method) ?? []), handler]),
  };
}

/** Start headless Chromium on a profile of its own and connect to it. */
export async function launchBrowser({ executable, profileDir, env, logFile, width, height }) {
  const log = openSync(logFile, 'a');
  const child = spawn(
    executable,
    [
      '--headless',
      '--remote-debugging-port=0',
      `--user-data-dir=${profileDir}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-background-networking',
      '--disable-component-update',
      '--disable-sync',
      '--disable-extensions',
      `--window-size=${width},${height}`,
      'about:blank',
    ],
    { env, stdio: ['ignore', log, 'pipe'] }
  );
  const endpoint = await new Promise((resolve, reject) => {
    let seen = '';
    const timer = setTimeout(
      () => reject(new Error(`Chromium did not report a DevTools endpoint: ${seen.slice(-400)}`)),
      60_000
    );
    child.stderr.on('data', (chunk) => {
      seen += chunk;
      const m = seen.match(/DevTools listening on (ws:\/\/\S+)/);
      if (m) {
        clearTimeout(timer);
        resolve(m[1]);
      }
    });
    child.once('exit', (code) =>
      reject(new Error(`Chromium exited (${code}) before it was ready: ${seen.slice(-400)}`))
    );
  });
  const socket = new WebSocket(endpoint);
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true });
    socket.addEventListener('error', () => reject(new Error('could not connect to Chromium')), {
      once: true,
    });
  });
  const { send, on } = connection(socket);
  return {
    pid: child.pid,
    send,
    on,
    /** A new tab; returns a `send` bound to it. */
    async newPage() {
      const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
      const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
      return { sessionId, send: (method, params) => send(method, params, sessionId) };
    },
    close() {
      try {
        socket.close();
      } catch {
        /* already closed */
      }
      child.kill('SIGTERM');
    },
  };
}
