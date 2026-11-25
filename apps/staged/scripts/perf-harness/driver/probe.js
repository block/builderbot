// The page probe: runs inside the app's page (injected, never bundled) and
// records what the main thread and the screen do. It only observes and
// appends to arrays; the driver does the arithmetic.
//
//   window.__stagedPerf.reset()    start a measurement window
//   window.__stagedPerf.collect()  what was recorded since
(() => {
  if (window.__stagedPerf) return;
  // What the fake agent writes into every piece of output: kind (x text,
  // t tool call), session tag, sequence number, epoch milliseconds. The
  // output of a tool call in flight is stamped `o` and is not read here: it
  // shows in no subtitle, and in a chat it is not the latency of anything.
  const STAMP = /pfm-([xt])([a-z0-9]+)-(\d+)-(\d{13})/g;
  const SCAN_TAIL = 65536;
  const fresh = () => ({ subtitle: { x: [], t: [] }, chat: { x: [], t: [] } });
  const state = {
    gaps: [],
    longTasks: [],
    timerLag: [],
    latency: fresh(),
    frameLatency: fresh(),
    mutations: 0,
    ipc: {},
  };
  // The newest sequence number shown per surface, kind and session. Kept
  // across resets, so output already on screen is never counted as new.
  const shown = new Map();

  // Frame gaps: the time between animation frames.
  let lastFrame = performance.now();
  const frame = (now) => {
    state.gaps.push(now - lastFrame);
    lastFrame = now;
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);

  // Long tasks, where the engine reports them (Chromium does, WebKit does not).
  const longTaskSupported = (PerformanceObserver.supportedEntryTypes ?? []).includes('longtask');
  if (longTaskSupported) {
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) state.longTasks.push(entry.duration);
    }).observe({ entryTypes: ['longtask'] });
  }
  // Timer lag: how late a 50 ms timer fires. The main thread's delay as any
  // engine shows it, and the only such figure in WebKit.
  let due = performance.now() + 50;
  setInterval(() => {
    const now = performance.now();
    state.timerLag.push(Math.max(0, now - due));
    due = now + 50;
  }, 50);

  // Stamps reaching the DOM: the end of the path from an agent's write.
  // Every stamp is counted once, the first time it shows: a subtitle holds
  // one line, so one stamp; a chat shows them all, so a batch of chunks
  // arriving together counts each chunk's own wait.
  const scan = (text, element, found) => {
    if (!text || text.indexOf('pfm-') === -1) return;
    const surface =
      element && element.closest && element.closest('[role="dialog"]') ? 'chat' : 'subtitle';
    const tail = text.length > SCAN_TAIL ? text.slice(-SCAN_TAIL) : text;
    STAMP.lastIndex = 0;
    for (let m = STAMP.exec(tail); m; m = STAMP.exec(tail)) {
      const key = `${surface}:${m[1]}:${m[2]}`;
      const seq = Number(m[3]);
      if (seq > (shown.get(key) ?? 0))
        found.set(`${key}:${seq}`, { key, surface, kind: m[1], seq, sent: Number(m[4]) });
    }
  };
  new MutationObserver((records) => {
    state.mutations += records.length;
    const found = new Map();
    for (const record of records) {
      if (record.type === 'characterData')
        scan(record.target.data, record.target.parentElement, found);
      else
        for (const node of record.addedNodes)
          scan(
            node.nodeType === 3 ? node.data : node.textContent,
            node.nodeType === 3 ? node.parentElement : node,
            found
          );
    }
    if (found.size === 0) return;
    const now = Date.now();
    for (const stamp of found.values()) {
      shown.set(stamp.key, Math.max(shown.get(stamp.key) ?? 0, stamp.seq));
      state.latency[stamp.surface][stamp.kind].push(now - stamp.sent);
    }
    // And once more at the next frame, which is when it can be seen.
    requestAnimationFrame(() => {
      const painted = Date.now();
      for (const stamp of found.values())
        state.frameLatency[stamp.surface][stamp.kind].push(painted - stamp.sent);
    });
  }).observe(document, { subtree: true, childList: true, characterData: true });

  // Desktop only: Tauri sends each command as a fetch to ipc://localhost/<command>.
  if (window.__TAURI_INTERNALS__) {
    const PREFIX = 'ipc://localhost/';
    const real = window.fetch;
    window.fetch = function (input) {
      const url = String(typeof input === 'string' ? input : (input && input.url) || '');
      const sent = real.apply(this, arguments);
      if (!url.startsWith(PREFIX)) return sent;
      const entry = (state.ipc[decodeURIComponent(url.slice(PREFIX.length))] ??= {
        count: 0,
        bytes: 0,
      });
      entry.count += 1;
      return sent.then((response) => {
        // A copy read as bytes: sized without parsing it a second time.
        response
          .clone()
          .arrayBuffer()
          .then((buffer) => (entry.bytes += buffer.byteLength))
          .catch(() => {});
        return response;
      });
    };
  }

  window.__stagedPerf = {
    reset() {
      state.gaps.length = 0;
      state.longTasks.length = 0;
      state.timerLag.length = 0;
      state.latency = fresh();
      state.frameLatency = fresh();
      state.mutations = 0;
      state.ipc = {};
      lastFrame = performance.now();
      due = performance.now() + 50;
    },
    collect() {
      return {
        ...state,
        longTaskSupported,
        hidden: document.hidden,
        tauri: !!window.__TAURI_INTERNALS__,
      };
    },
    /** How old the newest stamp on screen is, per surface: for catching up after a hidden window. */
    newestAge() {
      const out = {};
      const now = Date.now();
      for (const root of [document.querySelector('[role="dialog"]'), document.body]) {
        if (!root) continue;
        const text = root.innerText.slice(-SCAN_TAIL);
        let newest = 0;
        STAMP.lastIndex = 0;
        for (let m = STAMP.exec(text); m; m = STAMP.exec(text))
          newest = Math.max(newest, Number(m[4]));
        out[root === document.body ? 'page' : 'chat'] = newest ? now - newest : null;
      }
      return out;
    },
  };
})();
