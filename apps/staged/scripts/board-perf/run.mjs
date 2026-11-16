#!/usr/bin/env node
// Board performance harness: the frontend half of the kanban board baseline.
//
// Serves a production build of the app, opens it in a headless browser with
// mock-tauri.js standing in for the Tauri IPC bridge, and answers every
// backend call from the fixtures the backend benchmark wrote
// (src-tauri/src/board_load_bench.rs). Nothing reads ~/.staged: the page has
// no backend, and each run uses a fresh browser context.
//
//   node run.mjs --dist <vite build dir> --fixtures <dir with fixture-<n>.json>
//     [--sizes 50,200,1000] [--views home-grid,home-board,pinned-board]
//     [--browser chromium|webkit] [--rounds 3] [--out results.json]
//     [--profile <dir>]
//
// --profile (Chromium only) also writes a CPU profile of the load and of
// every update to <dir>, for attribute.py. Sampling slows the page down, so
// take timings from a run without it, and use a `vite build --minify false`
// dist so the profile carries function names.
//
// PLAYWRIGHT_CORE is the path of a playwright-core package (it is not a
// dependency of this repo); BROWSER_PATH overrides the browser executable.
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { readFileSync, existsSync, writeFileSync, statSync, mkdirSync } from 'node:fs';
import { extname, join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadavg } from 'node:os';

const here = dirname(fileURLToPath(import.meta.url));
const args = Object.fromEntries(
  process.argv.slice(2).reduce((pairs, arg, i, all) => {
    if (arg.startsWith('--')) pairs.push([arg.slice(2), all[i + 1]]);
    return pairs;
  }, [])
);
const dist = resolve(args.dist ?? 'dist');
const fixturesDir = resolve(args.fixtures ?? '.');
const sizes = (args.sizes ?? '50,200,1000').split(',').map(Number);
const views = (args.views ?? 'home-grid,home-board,pinned-board').split(',');
const browserName = args.browser ?? 'chromium';
const rounds = Number(args.rounds ?? 3);
const profileDir = args.profile ? resolve(args.profile) : null;
if (profileDir) mkdirSync(profileDir, { recursive: true });
const SETTLE_TIMEOUT_MS = 300_000;
const QUIET_MS = 500;

const require = createRequire(import.meta.url);
const playwright = require(process.env.PLAYWRIGHT_CORE ?? 'playwright-core');

const MIME = {
  '.html': 'text/html',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.woff2': 'font/woff2',
  '.wasm': 'application/wasm',
};
const server = createServer((req, res) => {
  const path = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  let file = join(dist, path);
  if (!file.startsWith(dist) || !existsSync(file) || statSync(file).isDirectory()) {
    file = join(dist, 'index.html');
  }
  res.writeHead(200, { 'Content-Type': MIME[extname(file)] ?? 'application/octet-stream' });
  res.end(readFileSync(file));
});
await new Promise((done) => server.listen(0, '127.0.0.1', done));
const url = `http://127.0.0.1:${server.address().port}/`;

const mockScript = readFileSync(join(here, 'mock-tauri.js'), 'utf8');
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const median = (values) => {
  const sorted = values.filter((v) => v !== null && v !== undefined).sort((a, b) => a - b);
  return sorted.length ? sorted[Math.floor(sorted.length / 2)] : null;
};

async function cdpMetrics(cdp) {
  if (!cdp) return null;
  const { metrics } = await cdp.send('Performance.getMetrics');
  const pick = (name) => (metrics.find((m) => m.name === name)?.value ?? 0) * 1000;
  return {
    task: pick('TaskDuration'),
    script: pick('ScriptDuration'),
    layout: pick('LayoutDuration'),
    style: pick('RecalcStyleDuration'),
  };
}

/**
 * Wait until every reply is in and frames have run freely for `quiet` ms.
 * `hydrated` also waits until that many projects have been given their
 * branch list, by whichever command the app reads it with, and `needCards`
 * until every card is in the document. Nothing here depends on how many
 * calls the app makes, so the harness runs unchanged against a fix.
 */
async function settle(page, { hydrated = 0, needCards = false, since = 0, quiet = QUIET_MS } = {}) {
  const deadline = Date.now() + SETTLE_TIMEOUT_MS;
  for (;;) {
    const snap = await page.evaluate(() => window.__boardPerf.snapshot());
    const last = Math.max(snap.lastReplyAt, snap.lastBusyEnd, since);
    if (
      snap.pending === 0 &&
      snap.hydrated >= hydrated &&
      (!needCards || snap.allCardsAt !== null) &&
      snap.now - last > quiet
    ) {
      return snap;
    }
    if (Date.now() > deadline) throw new Error(`not settled: ${JSON.stringify(snap.counts)}`);
    await sleep(100);
  }
}

const totalCalls = (counts) => Object.values(counts).reduce((sum, n) => sum + n, 0);

async function startProfile(cdp) {
  if (!profileDir || !cdp) return;
  await cdp.send('Profiler.enable');
  await cdp.send('Profiler.setSamplingInterval', { interval: 200 });
  await cdp.send('Profiler.start');
}

async function stopProfile(cdp, name) {
  if (!profileDir || !cdp) return;
  const { profile } = await cdp.send('Profiler.stop');
  const file = `${name.replace(/[^a-z0-9]+/gi, '-').replace(/^-|-$/g, '')}.cpuprofile`;
  writeFileSync(join(profileDir, file), JSON.stringify(profile));
}

/** Run `action`, wait for the app to go quiet, and report what it cost. */
async function measure(page, cdp, name, action, settleOptions = {}) {
  await startProfile(cdp);
  const before = await cdpMetrics(cdp);
  const t0 = await page.evaluate(() => window.__boardPerf.reset());
  const started = Date.now();
  const paintMs = await action();
  const actionMs = Date.now() - started;
  const snap = await settle(page, { ...settleOptions, since: t0 });
  const after = await cdpMetrics(cdp);
  await stopProfile(cdp, name);
  const lastWork = Math.max(snap.lastReplyAt, snap.lastBusyEnd);
  return {
    // To the second frame after the action, timed in the page when it can be.
    paintMs: paintMs ?? actionMs,
    // Until the last reply or the last blocked frame.
    quietMs: Math.max(lastWork - t0, paintMs ?? 0),
    blockedMs: snap.blockedMs,
    longestFrameMs: snap.longestFrameMs,
    calls: totalCalls(snap.counts),
    counts: snap.counts,
    mainThreadMs: after ? after.task - before.task : null,
    scriptMs: after ? after.script - before.script : null,
    styleLayoutMs: after ? after.style + after.layout - before.style - before.layout : null,
  };
}

/** Run a function in the page and time it to the second frame after it. */
const timedInPage = (page, fn, arg) =>
  page.evaluate(
    async ({ source, arg }) => {
      const run = new Function(`return (${source})`)();
      const start = performance.now();
      await run(arg);
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return performance.now() - start;
    },
    { source: fn.toString(), arg }
  );

const emit = (page, event, payload) =>
  timedInPage(page, ({ event, payload }) => window.__boardPerf.emit(event, payload), {
    event,
    payload,
  });

/** The search field of the view on top (the home page's stays mounted under a project). */
const typeQuery = (page, value) =>
  timedInPage(
    page,
    (value) => {
      const fields = [...document.querySelectorAll('input[placeholder^="Search projects,"]')];
      const field = fields.findLast((el) => el.getClientRects().length > 0);
      field.value = value;
      field.dispatchEvent(new Event('input', { bubbles: true }));
    },
    value
  );

async function runOnce(browser, n, view) {
  const fixture = JSON.parse(readFileSync(join(fixturesDir, `fixture-${n}.json`), 'utf8'));
  const statusGroup = fixture.tagState.groups.find((group) => group.showInSidebar);
  const statusTags = fixture.tagState.tags.filter((tag) => tag.groupId === statusGroup.id);
  const prefs = {
    'projects-view-mode': view === 'home-board' ? 'board' : 'grid',
    'projects-board-group': statusGroup.id,
  };
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    serviceWorkers: 'block',
  });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(String(error)));
  await page.addInitScript(
    `window.__BOARD_PERF_INPUT__ = ${JSON.stringify({ fixture, prefs, expectedCards: n })};`
  );
  await page.addInitScript(mockScript);
  const cdp = browserName === 'chromium' ? await context.newCDPSession(page) : null;
  if (cdp) await cdp.send('Performance.enable');

  const result = { n, view, updates: {} };
  const label = (update) => `${n}-${view}-${update}`;
  await startProfile(cdp);
  await page.goto(url);
  const loaded = await settle(page, { hydrated: n, needCards: true });
  const loadCdp = await cdpMetrics(cdp);
  await stopProfile(cdp, label('load'));
  const load = {
    firstCardMs: loaded.firstCardAt,
    allCardsMs: loaded.allCardsAt,
    quietMs: Math.max(loaded.lastReplyAt, loaded.lastBusyEnd, loaded.allCardsAt),
    blockedMs: loaded.blockedMs,
    longestFrameMs: loaded.longestFrameMs,
    calls: totalCalls(loaded.counts),
    counts: loaded.counts,
    mainThreadMs: loadCdp?.task ?? null,
    scriptMs: loadCdp?.script ?? null,
    styleLayoutMs: loadCdp ? loadCdp.style + loadCdp.layout : null,
  };
  result.misses = loaded.misses;

  const project = fixture.projects[0];
  const branch = fixture.branches[project.id][0];
  const boardCards = (count) =>
    page.waitForFunction(
      (count) => document.querySelectorAll('[data-board-card]').length >= count,
      count,
      { polling: 'raf', timeout: SETTLE_TIMEOUT_MS }
    );

  if (view === 'pinned-board') {
    // The home grid stays mounted under the project layer, as in the app.
    await page.locator('.project-card').first().click();
    await page.locator(`[title="${statusGroup.name} board"]`).waitFor();
    await settle(page);
    result.updates['open pinned board'] = await measure(
      page,
      cdp,
      label('open pinned board'),
      async () => {
        const start = Date.now();
        await page.locator(`[title="${statusGroup.name} board"]`).click();
        await boardCards(n);
        return Date.now() - start;
      }
    );
  } else {
    result.load = load;
  }

  const updates = result.updates;
  if (view === 'home-grid') {
    updates['switch grid to board'] = await measure(
      page,
      cdp,
      label('switch grid to board'),
      async () => {
        const start = Date.now();
        await page.getByRole('radio', { name: 'Board' }).click();
        await boardCards(n);
        return Date.now() - start;
      }
    );
    updates['switch board to grid'] = await measure(
      page,
      cdp,
      label('switch board to grid'),
      async () => {
        const start = Date.now();
        await page.getByRole('radio', { name: 'Grid' }).click();
        await page.waitForFunction(() => !document.querySelector('[data-board-card]'), null, {
          polling: 'raf',
        });
        return Date.now() - start;
      }
    );
  }

  updates['pr-status-changed (one branch)'] = await measure(
    page,
    cdp,
    label('pr-status-changed (one branch)'),
    () =>
      emit(page, 'pr-status-changed', {
        branchId: branch.id,
        prState: 'OPEN',
        prChecksStatus: 'PENDING',
        prReviewDecision: null,
        prMergeable: true,
        prDraft: false,
        prHeadSha: 'f'.repeat(40),
        prFetchedAt: 1,
        failedChecks: [],
      })
  );
  updates['branch-changed (one branch)'] = await measure(
    page,
    cdp,
    label('branch-changed (one branch)'),
    () => emit(page, 'branch-changed', { branchId: branch.id, projectId: project.id })
  );
  updates['tags-changed (one project retagged elsewhere)'] = await measure(
    page,
    cdp,
    label('tags-changed (one project retagged elsewhere)'),
    () => {
      return timedInPage(
        page,
        ({ projectId, groupTagIds, tagId }) => {
          const perf = window.__boardPerf;
          const state = perf.fixture.tagState;
          state.assignments = state.assignments.filter(
            (a) => !(a.projectId === projectId && groupTagIds.includes(a.tagId))
          );
          state.assignments.push({ projectId, tagId });
          perf.invalidate();
          perf.emit('tags-changed', { projectIds: [projectId] });
        },
        {
          projectId: fixture.projects[1].id,
          groupTagIds: statusTags.map((tag) => tag.id),
          tagId: statusTags[3].id,
        }
      );
    }
  );
  updates['search: type a query'] = await measure(page, cdp, label('search: type a query'), () =>
    typeQuery(page, 'ledger')
  );
  updates['search: clear the query'] = await measure(
    page,
    cdp,
    label('search: clear the query'),
    () => typeQuery(page, '')
  );

  if (view !== 'home-grid') {
    // A card moved the keyboard way: its context menu, Move to, a column.
    const card = page.locator(`[data-board-card="${project.id}"]`);
    await card.scrollIntoViewIfNeeded();
    updates['open a card menu'] = await measure(page, cdp, label('open a card menu'), async () => {
      const start = Date.now();
      await card.click({ button: 'right' });
      await page.locator('[data-tag-group-trigger="move-to"]').waitFor();
      return Date.now() - start;
    });
    const current = fixture.tagState.assignments.find(
      (a) => a.projectId === project.id && statusTags.some((tag) => tag.id === a.tagId)
    );
    const target = statusTags.find((tag) => tag.id !== current?.tagId && tag.name !== 'Backlog');
    await page.locator('[data-tag-group-trigger="move-to"]').click();
    const item = page.getByRole('menuitemradio', { name: target.name });
    await item.waitFor();
    updates['move a card to another column'] = await measure(
      page,
      cdp,
      label('move a card to another column'),
      async () => {
        const start = Date.now();
        await item.click();
        await page
          .locator(`[data-board-column="${target.id}"] [data-board-card="${project.id}"]`)
          .waitFor();
        return Date.now() - start;
      }
    );
  }

  updates['project-changed (any project write)'] = await measure(
    page,
    cdp,
    label('project-changed (any project write)'),
    () => emit(page, 'project-changed', { projectId: project.id }),
    // The reload this starts is paced by idle time, so allow for longer gaps.
    { quiet: 2000 }
  );

  result.errors = errors;
  await context.close();
  return result;
}

const browser = await playwright[browserName].launch({
  headless: true,
  executablePath: process.env.BROWSER_PATH || undefined,
});
const runs = [];
for (const n of sizes) {
  for (const view of views) {
    for (let round = 0; round < rounds; round++) {
      const started = Date.now();
      const run = await runOnce(browser, n, view);
      run.round = round;
      run.load1 = loadavg()[0];
      runs.push(run);
      const head = run.load
        ? `first card ${run.load.firstCardMs.toFixed(0)} ms, quiet ${run.load.quietMs.toFixed(0)} ms, ${run.load.calls} calls`
        : `open ${run.updates['open pinned board'].paintMs.toFixed(0)} ms`;
      console.error(
        `[${browserName}] n=${n} ${view} round ${round}: ${head}; ${Date.now() - started} ms total; load ${run.load1.toFixed(1)}; misses ${JSON.stringify(run.misses)}; errors ${run.errors.length}`
      );
    }
  }
}
await browser.close();
server.close();

// ── Report: the median round per dataset size and view ──
const fmt = (value) =>
  value === null || value === undefined ? 'n/a' : value.toFixed(value < 10 ? 1 : 0);
const group = (n, view) => runs.filter((run) => run.n === n && run.view === view);
const med = (set, pick) => median(set.map(pick));
const perProject = (counts) =>
  (counts.list_branches_for_project ?? 0) + (counts.list_project_repos ?? 0);

console.log(`\n### Load (${browserName}, median of ${rounds})\n`);
console.log(
  '| projects | view | backend calls | per-project calls | first card ms | all cards ms | quiet ms | frames blocked ms | longest frame ms | main thread ms | script ms | style+layout ms |'
);
console.log('|---|---|---|---|---|---|---|---|---|---|---|---|');
for (const n of sizes) {
  for (const view of views) {
    const set = group(n, view).filter((run) => run.load);
    if (!set.length) continue;
    const m = (key) => fmt(med(set, (run) => run.load[key]));
    console.log(
      `| ${n} | ${view} | ${m('calls')} | ${fmt(med(set, (run) => perProject(run.load.counts)))} | ${m('firstCardMs')} | ${m('allCardsMs')} | ${m('quietMs')} | ${m('blockedMs')} | ${m('longestFrameMs')} | ${m('mainThreadMs')} | ${m('scriptMs')} | ${m('styleLayoutMs')} |`
    );
  }
}

console.log(`\n### Updates (${browserName}, median of ${rounds})\n`);
console.log(
  '| projects | view | update | backend calls | to next paint ms | quiet ms | frames blocked ms | longest frame ms | main thread ms | script ms | style+layout ms |'
);
console.log('|---|---|---|---|---|---|---|---|---|---|---|');
for (const n of sizes) {
  for (const view of views) {
    const set = group(n, view);
    if (!set.length) continue;
    for (const name of Object.keys(set[0].updates)) {
      const m = (key) => fmt(med(set, (run) => run.updates[name]?.[key]));
      console.log(
        `| ${n} | ${view} | ${name} | ${m('calls')} | ${m('paintMs')} | ${m('quietMs')} | ${m('blockedMs')} | ${m('longestFrameMs')} | ${m('mainThreadMs')} | ${m('scriptMs')} | ${m('styleLayoutMs')} |`
      );
    }
  }
}

if (args.out) {
  writeFileSync(
    args.out,
    JSON.stringify({ browser: browserName, sizes, views, rounds, runs }, null, 2)
  );
}
