// A minimal Chrome DevTools Protocol client, so the harness needs a Chromium
// executable and no npm dependency.

import { spawn } from 'node:child_process';
import { openSync } from 'node:fs';

/** Start headless Chromium on a profile of its own and connect to it. */
export async function launchBrowser({ executable, profileDir, env, logFile }) {
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
      '--window-size=1500,1000',
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
  let nextId = 0;
  const pending = new Map();
  const listeners = new Map();
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    if (message.id !== undefined) {
      const call = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) call?.reject(new Error(`${call.method}: ${message.error.message}`));
      else call?.resolve(message.result);
      return;
    }
    for (const handler of listeners.get(message.method) ?? [])
      handler(message.params, message.sessionId);
  });
  const send = (method, params = {}, sessionId) =>
    new Promise((resolve, reject) => {
      const id = ++nextId;
      pending.set(id, { resolve, reject, method });
      socket.send(JSON.stringify({ id, method, params, sessionId }));
    });
  return {
    pid: child.pid,
    send,
    on: (method, handler) => listeners.set(method, [...(listeners.get(method) ?? []), handler]),
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
