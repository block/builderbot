// The two frontends a run can measure, behind one interface:
//
//   evaluate(body)   run the body of an async function in the page
//   reload()         load the app afresh, with the probe and the overrides
//   reset()          start a measurement window
//   collect()        the page probe's record, plus what only this frontend sees
//   close()
//
// `browser` is headless Chromium on the loopback server (the web transport:
// fetch and a WebSocket). `desktop` is the app's own webview (Tauri IPC and
// channels), reached through the harness's eval route.

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { TOKEN_HEADER, tokenCookie, tokenCookieScript } from './app.mjs';
import { launchBrowser } from './cdp.mjs';
import { sleep, until } from './util.mjs';

const PROBE = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'probe.js'), 'utf8');

const VIEWPORT_WIDTH = 1500;

/** Sets the run's localStorage overrides before any app code reads them. */
const overridesScript = (overrides) =>
  `(() => { try { for (const [key, value] of Object.entries(${JSON.stringify(overrides)})) localStorage.setItem(key, value); } catch (_) {} })();`;

const asyncBody = (body) => `(async () => { ${body} })()`;

/**
 * Whether the document is the app's refusal of a caller without the run's
 * token (`perf_harness/server.rs` answers 401 with this text) and not the
 * app's page.
 */
const REFUSED = `(document.body ? document.body.textContent : '').startsWith('perf-harness:')`;

/**
 * Set on the window of a document that is about to be loaded again, so the
 * document that replaces it (a new window, without the mark) can be told
 * from it. The desktop webview's first document is always a refusal: it is
 * what the token's cookie is set from.
 */
const LEAVING = '__stagedPerfLeaving';

/**
 * Where the desktop webview's load stands: `'ready'` once the app's page is
 * loaded with the probe in it, `'refused'` when the document loaded is the
 * app's refusal, and `null` while there is nothing to say yet (the old
 * document is still there, or the new one is still loading).
 */
export const desktopLoadState = `
  const loaded = document.readyState === 'complete' && location.protocol.startsWith('http') && !window.${LEAVING};
  if (!loaded) return null;
  if (${REFUSED}) return 'refused';
  return window.__stagedPerf ? 'ready' : null;`;

export async function openBrowserFrontend({
  api,
  executable,
  profileDir,
  env,
  logFile,
  localStorage,
  viewportHeight,
}) {
  const [width, height] = [VIEWPORT_WIDTH, viewportHeight];
  const browser = await launchBrowser({ executable, profileDir, env, logFile, width, height });
  const page = await browser.newPage();
  const network = networkRecorder(browser, page.sessionId);
  await page.send('Page.enable');
  await page.send('Runtime.enable');
  await page.send('Network.enable');
  await page.send('Performance.enable');
  // The build ships a service worker; a measured page must not be served by one.
  await page.send('Network.setBypassServiceWorker', { bypass: true });
  await page.send('Emulation.setDeviceMetricsOverride', {
    width,
    height,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await page.send('Page.addScriptToEvaluateOnNewDocument', {
    source: `${overridesScript(localStorage)}\n${PROBE}`,
  });
  // The app serves this page nothing without the run's token, and the page
  // is the app's own code, which knows of no token: the browser is given it
  // as a cookie before the page first loads.
  const { success } = await page.send('Network.setCookie', tokenCookie(api));
  if (!success) throw new Error('the browser did not take the cookie that holds the run’s token');
  const evaluate = async (body) => {
    const { result, exceptionDetails } = await page.send('Runtime.evaluate', {
      expression: asyncBody(body),
      awaitPromise: true,
      returnByValue: true,
    });
    if (exceptionDetails)
      throw new Error(exceptionDetails.exception?.description ?? exceptionDetails.text);
    return result.value;
  };
  const cpu = async () =>
    Object.fromEntries(
      (await page.send('Performance.getMetrics')).metrics.map((m) => [m.name, m.value])
    );
  let cpuAtReset = null;
  return {
    kind: 'browser',
    pid: browser.pid,
    evaluate,
    async reload() {
      await page.send('Page.navigate', { url: `${api.base}/` });
      await until(
        'the page to load',
        () => evaluate(`return document.readyState === 'complete' && !!window.__stagedPerf;`),
        { timeout: 120_000 }
      );
      if (await evaluate(`return ${REFUSED};`))
        throw new Error('the app refused the page: the browser did not send the run’s token');
    },
    async reset() {
      await evaluate('window.__stagedPerf.reset(); return true;');
      network.reset();
      cpuAtReset = await cpu();
    },
    async collect() {
      const probe = await evaluate('return window.__stagedPerf.collect();');
      const now = await cpu();
      const delta = (name) => now[name] - cpuAtReset[name];
      return {
        probe,
        network: network.take(),
        // The renderer's own accounting of where its main thread's time went.
        renderer: {
          taskSeconds: delta('TaskDuration'),
          scriptSeconds: delta('ScriptDuration'),
          layoutSeconds: delta('LayoutDuration'),
          styleSeconds: delta('RecalcStyleDuration'),
          heapUsedMb: now.JSHeapUsedSize / 1048576,
          domNodes: now.Nodes,
        },
      };
    },
    close: () => browser.close(),
  };
}

/** Calls, events and stream frames as the page receives them, from the DevTools network events. */
function networkRecorder(browser, sessionId) {
  const requests = new Map();
  // Whether any WebSocket of the page negotiated compression. Decided at the
  // handshake, before the first window, so it is not part of the record.
  let socketCompressed = false;
  let record;
  const reset = () => (record = { commands: {}, events: {}, stream: {} });
  reset();
  const mine = (handler) => (params, from) => from === sessionId && handler(params);
  browser.on(
    'Network.requestWillBeSent',
    mine(({ requestId, request }) => {
      const m = request.url.match(/\/api\/invoke\/([\w-]+)/);
      if (!m) return;
      const entry = (record.commands[m[1]] ??= { count: 0, bytes: 0, wireBytes: 0 });
      entry.count += 1;
      requests.set(requestId, entry);
    })
  );
  browser.on(
    'Network.dataReceived',
    mine(({ requestId, dataLength }) => {
      const entry = requests.get(requestId);
      if (entry) entry.bytes += dataLength;
    })
  );
  browser.on(
    'Network.loadingFinished',
    mine(({ requestId, encodedDataLength }) => {
      const entry = requests.get(requestId);
      if (entry) entry.wireBytes += encodedDataLength;
      requests.delete(requestId);
    })
  );
  browser.on(
    'Network.webSocketHandshakeResponseReceived',
    mine(({ response }) => {
      const extensions = Object.entries(response.headers ?? {}).find(
        ([name]) => name.toLowerCase() === 'sec-websocket-extensions'
      );
      if (extensions && /permessage-deflate/i.test(extensions[1])) socketCompressed = true;
    })
  );
  browser.on(
    'Network.webSocketFrameReceived',
    mine(({ response }) => {
      const text = response.payloadData;
      const head = text.slice(0, 600);
      const name = (head.match(/"event":"([^"]+)"/) ?? [])[1] ?? 'unknown';
      const bytes = Buffer.byteLength(text);
      const event = (record.events[name] ??= { frames: 0, bytes: 0 });
      event.frames += 1;
      event.bytes += bytes;
      if (name !== 'session-stream') return;
      const level = head.includes('"full":')
        ? 'full'
        : head.includes('"summary":')
          ? 'summary'
          : 'none';
      const kind = (head.match(/"kind":"(\w+)"/) ?? [])[1] ?? 'unknown';
      const frames = (record.stream[`${level}.${kind}`] ??= { frames: 0, bytes: 0 });
      frames.frames += 1;
      frames.bytes += bytes;
    })
  );
  // A copy: the live record keeps counting after the window closes.
  return { reset, take: () => ({ ...structuredClone(record), socketCompressed }) };
}

export async function openDesktopFrontend({ api, localStorage }) {
  let busy = Promise.resolve();
  // One script at a time: its answer comes back through a shared queue.
  const evaluate = (body) => {
    const run = async () => {
      await api.takeResults();
      const script = `(async () => { let out; try { out = await ${asyncBody(body)}; } catch (e) { out = { __error: String((e && e.stack) || e) }; }
        await fetch('/perf/result', { method: 'POST', headers: { ${JSON.stringify(TOKEN_HEADER)}: ${JSON.stringify(api.token)} }, body: JSON.stringify(out ?? null) }); })();`;
      const windows = await api.evalInWebviews(script);
      if (windows.length === 0) throw new Error('the app has no webview to run a script in');
      const [answer] = await until('a script to answer from the webview', async () => {
        const got = await api.takeResults();
        return got.length ? got : null;
      });
      const value = JSON.parse(answer);
      if (value && value.__error) throw new Error(value.__error);
      return value;
    };
    const next = busy.then(run, run);
    busy = next.catch(() => {});
    return next;
  };
  return {
    kind: 'desktop',
    pid: null,
    evaluate,
    async reload() {
      // The webview's first load was refused: it had no token. The
      // document that refusal left is of the app's origin, so it can set
      // the cookie the next load is served for. The overrides are set, then
      // the page is loaded again so the app boots with them; the probe goes
      // in as soon as the document exists.
      await api.evalInWebviews(
        `${tokenCookieScript(api)} ${overridesScript(localStorage)} window.${LEAVING} = true; location.reload();`
      );
      await sleep(1500);
      const state = await until(
        'the webview to load',
        async () => {
          await api.evalInWebviews(PROBE);
          return evaluate(desktopLoadState);
        },
        { timeout: 120_000, every: 1000 }
      );
      // Refused again, with the cookie set: waiting longer changes nothing,
      // so say what happened, as the browser frontend does.
      if (state === 'refused')
        throw new Error('the app refused the page: the webview did not send the run’s token');
      // The window is never put on screen, so the page starts out hidden.
      await evaluate(`
        Object.defineProperty(document, 'hidden', { configurable: true, get: () => false });
        Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => 'visible' });
        document.dispatchEvent(new Event('visibilitychange'));
        return true;`);
    },
    reset: () => evaluate('window.__stagedPerf.reset(); return true;'),
    collect: async () => ({
      probe: await evaluate('return window.__stagedPerf.collect();'),
      network: null,
      renderer: null,
    }),
    close() {},
  };
}
