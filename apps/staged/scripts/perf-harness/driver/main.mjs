// The driver: runs from inside the throwaway directory with the scrubbed
// environment `run.mjs` gave it. Starts the app, seeds it, opens the
// frontend, runs the scenarios and writes `out/result.json`.
//
//   node main.mjs <config.json>
//
// The result file is written as soon as the app is up and again after
// every scenario, so a run that fails or is interrupted keeps what it had
// measured. A failure is recorded in the file (`failure`, and on the
// scenario it happened in) and ends the run: what state the app is in
// after one is not known, so the scenarios after it are listed as not run.

import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { loadavg } from 'node:os';
import { join } from 'node:path';

import { listProcesses, tree } from '../lib/processes.mjs';
import { isInside, resolveExisting } from '../lib/safety.mjs';
import { createClient, launchApp } from './app.mjs';
import { openBrowserFrontend, openDesktopFrontend } from './frontend.mjs';
import { agentLoad, backendResult, frontendResult, mountedRows } from './measure.mjs';
import { expectedChunksPerSecond, overloadReason } from './overload.mjs';
import { startSampler } from './sampler.mjs';
import { plan, SCENARIOS } from './scenarios.mjs';
import { seed } from './seed.mjs';
import { startStaleness } from './staleness.mjs';
import * as ui from './ui.mjs';
import { log, sleep, until } from './util.mjs';
import { verifyChat, verifySubtitles } from './verify.mjs';

const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const { runDir, home, params } = config;
const env = process.env;
let app;
let appExited = null;
let frontend;
let sampler;
let phase = 'starting the app';

/** The hooks version this driver was written against; see `HOOKS_VERSION` in the app's `perf_harness/metrics.rs`. */
const HOOKS_WANTED = 2;
/** A window is not judged overloaded on less than this much of it. */
const OVERLOAD_AFTER_S = 10;

/** A failure with something to put in the result beside its message. */
class Failure extends Error {
  constructor(message, detail) {
    super(message);
    this.detail = detail;
  }
}

const result = {
  schema: 2,
  complete: false,
  label: params.label,
  startedAt: config.startedAt,
  tree: config.tree,
  frontend: params.frontend,
  streamSupported: null,
  hooks: null,
  params,
  host: { platform: process.platform, loadAverageStart: loadavg(), loadAverageEnd: null },
  scenarios: {},
  notRun: [...params.scenarios],
  failure: null,
  checks: { ok: true, failed: [] },
  warnings: [],
  safety: null,
};

function writeResult(dataDir = result.safety?.dataDir ?? null) {
  result.host.loadAverageEnd = loadavg();
  result.safety = {
    runDir,
    dataDir,
    realStagedDir: config.realStaged,
    ...(sampler?.safety() ?? {
      processSamples: 0,
      openFileSamples: 0,
      openUnderRealStaged: [],
      unexpectedExecutables: [],
    }),
  };
  writeFileSync(join(runDir, 'out', 'result.json'), JSON.stringify(result, null, 2));
}

// If `run.mjs` dies without cleaning up, this process must not live on.
setInterval(() => {
  if (process.ppid !== config.parentPid) {
    stop();
    process.exit(1);
  }
}, 1000).unref();

function stop() {
  sampler?.stop();
  try {
    frontend?.close();
  } catch {
    /* gone */
  }
  app?.kill('SIGTERM');
}

function writeAgentConfig() {
  const shape = (chunksPerSecond) => ({
    chunksPerSecond,
    chunkChars: params.chunkChars,
    paragraphEvery: params.paragraphEvery,
    toolEvery: params.toolEvery,
    toolMs: params.toolMs,
    toolBytes: params.toolBytes,
    largeToolEvery: params.largeToolEvery,
    largeToolBytes: params.largeToolBytes,
    planEvery: params.planEvery,
    toolStepMs: params.toolStepMs,
    // A turn outlasts its scenario; the driver cancels it when the window closes.
    turnSeconds: params.warmup + params.duration + 300,
  });
  const profiles = { background: shape(params.rate), chat: shape(params.chatRate) };
  writeFileSync(config.agentConfig, JSON.stringify({ profiles }));
  return profiles;
}

/**
 * The opening prompt: the two words the fake agent reads, padded with
 * filler to `--prompt-bytes` when that is more.
 */
function promptFor(session) {
  const words = `Load test pf-tag=${session.tag} pf-profile=${session.profile}`;
  const line = 'Context the session was started with, which the agent has no use for.\n';
  const padding = params.promptBytes - words.length - 2;
  return padding > 0
    ? `${words}\n\n${line.repeat(Math.ceil(padding / line.length)).slice(0, padding)}`
    : words;
}

function agentState() {
  const dir = join(runDir, 'agent-state');
  const out = {};
  for (const file of readdirSync(dir)) {
    try {
      const state = JSON.parse(readFileSync(join(dir, file), 'utf8'));
      out[state.tag] = state;
    } catch {
      /* being rewritten */
    }
  }
  return out;
}

async function startApp() {
  mkdirSync(join(runDir, 'tmp'), { recursive: true });
  app = launchApp({
    binary: config.binary,
    cwd: runDir,
    logFile: join(runDir, 'logs', 'app.log'),
    env: {
      ...env,
      STAGED_ACP_TOOLS_DIR: join(runDir, 'acp-tools'),
      STAGED_PERF_HARNESS_PORT: String(config.port),
      STAGED_PERF_HARNESS_ROOT: runDir,
      STAGED_PERF_HARNESS_DIST: config.dist,
      STAGED_PERF_HARNESS_TOKEN: config.token,
    },
  });
  app.once(
    'exit',
    (code, signal) => (appExited = `the app exited (${signal ?? code}); see logs/app.log`)
  );
  const api = createClient({ port: config.port, token: config.token });
  const metrics = await until('the app to serve its harness port', () => api.metrics(), {
    timeout: 180_000,
    every: 500,
    giveUp: () => appExited,
  });
  // The app's own answer to where its data is, checked before anything is written through it.
  const dataDir = resolveExisting(metrics.dataDir);
  if (
    metrics.pid !== app.pid ||
    !isInside(dataDir, resolveExisting(runDir)) ||
    isInside(dataDir, config.realStaged)
  ) {
    throw new Error(
      `the process on port ${config.port} is not this run's app, or its data directory ${dataDir} is not inside ${runDir}`
    );
  }
  await until('the app to answer commands', () => api.invoke('list_projects'), {
    timeout: 120_000,
    every: 500,
    giveUp: () => appExited,
  });
  return { api, dataDir, hooks: metrics.hooks ?? 1 };
}

/**
 * Why each session that is not streaming is not: its row's status, how long
 * after its creation the row was last written, how many messages it has,
 * and what its agent has reported. A session that shows `running` with no
 * message and no agent turn is one whose start never finished in the app.
 */
async function notStreaming(api, sessions) {
  const state = agentState();
  const agents = tree(listProcesses(), app.pid).filter((p) => p.args.includes('fake-agent.cjs'));
  const alive = (pid) => agents.some((p) => p.pid === pid);
  const stuck = [];
  for (const session of sessions.filter((s) => !(state[s.tag]?.chunks > 0))) {
    const row = await api.invoke('get_session', { sessionId: session.id }).catch(() => null);
    const messages = await api
      .invoke('get_session_messages', { sessionId: session.id, limit: 200 })
      .catch(() => null);
    const turn = state[session.tag];
    stuck.push({
      tag: session.tag,
      sessionId: session.id,
      status: row?.status ?? 'unreadable',
      writtenAfterCreationMs: row ? row.updatedAt - row.createdAt : null,
      messages: messages?.length ?? null,
      agent: turn
        ? `turn started in pid ${turn.pid}, ${alive(turn.pid) ? 'running' : 'gone'}, no output yet`
        : 'no turn reported by any agent',
    });
  }
  return { stuck, agentProcesses: agents.length, sessions: sessions.length };
}

/** How many sessions a failure's message describes in full. */
const SESSIONS_NAMED = 3;

async function waitForStreaming(api, sessions) {
  try {
    await until(
      'every session to be streaming',
      () => {
        const state = agentState();
        return sessions.every((s) => state[s.tag]?.chunks > 0);
      },
      { timeout: params.startTimeout * 1000, every: 500, giveUp: () => appExited }
    );
  } catch (error) {
    if (appExited) throw error;
    const detail = await notStreaming(api, sessions);
    const lines = detail.stuck.map(
      (s) =>
        `${s.tag} (session ${s.sessionId}): status ${s.status}, ${s.messages ?? '?'} messages, row last written ${s.writtenAfterCreationMs ?? '?'} ms after it was created, ${s.agent}`
    );
    // The message names the first few; `failure.detail.stuck` has them all.
    const more = lines.length - SESSIONS_NAMED;
    throw new Failure(
      `${detail.stuck.length} of ${detail.sessions} sessions were not streaming after ${params.startTimeout}s (the app has ${detail.agentProcesses} agent processes): ${lines.slice(0, SESSIONS_NAMED).join('; ')}${more > 0 ? `; and ${more} more, listed in the result's failure.detail.stuck` : ''}`,
      { kind: 'start-timeout', ...detail }
    );
  }
}

/** Sleep through the measured window, stopping the run if it turns out to measure an overloaded machine. */
async function measureWindow({ from, sessions, agentsBefore, expectedPerSecond }) {
  const end = from + params.duration * 1000;
  const check = (seconds) => {
    const state = agentState();
    const chunks = sessions.reduce(
      (total, s) => total + ((state[s.tag]?.chunks ?? 0) - (agentsBefore[s.tag]?.chunks ?? 0)),
      0
    );
    const reason = overloadReason({
      chunks,
      expectedPerSecond,
      samples: sampler.between(from, Date.now()).app.length,
      seconds,
      minShare: params.minRateShare,
    });
    if (reason) {
      throw new Failure(
        `the machine is overloaded, so the window was not measured: ${reason}, ${Math.round((Date.now() - from) / 1000)}s into the window (--min-rate-share ${params.minRateShare})`,
        { kind: 'overload', reason, loadAverage: loadavg() }
      );
    }
  };
  for (;;) {
    const left = end - Date.now();
    if (left <= 0) break;
    await sleep(Math.min(2000, left));
    if (appExited) throw new Error(appExited);
    // The agents' counters and the sampler are each up to a second behind.
    const seconds = (Date.now() - from) / 1000 - 1;
    if (seconds >= OVERLOAD_AFTER_S) check(seconds);
  }
  check((Date.now() - from) / 1000 - 1);
}

async function runScenario({ name, index, api, projects, profiles, streamSupported }) {
  const scenario = SCENARIOS[name];
  const sessions = plan({ name, index, projects, running: params.running });
  const step = (what) => {
    phase = `scenario ${name}: ${what}`;
    log(phase);
  };
  step(`starting ${sessions.length} sessions`);
  for (const session of sessions) {
    const started = await api.invoke('start_or_queue_branch_session', {
      branchId: session.branchId,
      prompt: promptFor(session),
      sessionType: 'note',
      provider: 'claude',
      imageIds: null,
      launchContext: null,
      acpConfigSelection: null,
    });
    session.id = started.sessionId;
  }
  await waitForStreaming(api, sessions);

  const viewedSessions = sessions.filter((s) => s.project === 0);
  const viewed = viewedSessions[0];
  // What `--prompt-bytes` came to once the app had wrapped the prompt.
  const sessionRowBytes = Buffer.byteLength(
    JSON.stringify(await api.invoke('get_session', { sessionId: viewed.id }))
  );
  step('opening the view');
  await frontend.reload();
  await frontend.evaluate(ui.openProject(projects[0].name, viewed));
  if (scenario.chat) await frontend.evaluate(ui.openChat(viewed));
  await sleep(params.warmup * 1000);
  if (scenario.hidden) {
    await frontend.evaluate(ui.setHidden(true));
    await sleep(1000);
  }

  step(`measuring for ${params.duration}s`);
  await api.resetMetrics();
  await frontend.reset();
  const agentsBefore = agentState();
  const rowsAtStart = (await frontend.evaluate(ui.rows(viewedSessions))).rows;
  const loadAverageStart = loadavg()[0];
  const staleness = startStaleness({ api, frontend, sessions: viewedSessions });
  const from = Date.now();
  const expectedPerSecond = sessions.reduce(
    (total, s) => total + expectedChunksPerSecond(profiles[s.profile]),
    0
  );
  try {
    await measureWindow({ from, sessions, agentsBefore, expectedPerSecond });
  } catch (error) {
    await staleness.stop();
    throw error;
  }
  const to = Date.now();
  const seconds = (to - from) / 1000;
  const [metrics, collected, subtitleStaleness] = await Promise.all([
    api.metrics(),
    frontend.collect(),
    staleness.stop(),
  ]);
  const agentsAfter = agentState();
  const loadAverageEnd = loadavg()[0];
  await sampler.sampleNow();
  const usage = sampler.between(from, to);
  const rowsAtEnd = (await frontend.evaluate(ui.rows(viewedSessions))).rows;
  const shown = await frontend.evaluate(ui.describe());

  let catchUpMs = null;
  if (scenario.hidden) {
    step('showing the window again');
    const unhidden = Date.now();
    await frontend.evaluate(ui.setHidden(false));
    catchUpMs = await until(
      'the page to catch up after being shown',
      async () => {
        const age = await frontend.evaluate('return window.__stagedPerf.newestAge();');
        return age.chat !== null && age.chat < 2000 ? Date.now() - unhidden : null;
      },
      { timeout: 60_000, every: 100 }
    ).catch(() => null);
  }

  // The checks come after the window, so their store reads are in no figure:
  // the subtitles while the sessions still run, the chat once they have stopped.
  const checks = {};
  if (!params.skipVerify) {
    step('checking the subtitles against the store');
    checks.subtitles = await verifySubtitles({ api, frontend, sessions: viewedSessions });
  }
  step('stopping the sessions');
  for (const session of sessions)
    await api.invoke('cancel_session', { sessionId: session.id }).catch(() => null);
  await until(
    'the sessions to stop',
    async () => {
      const statuses = await Promise.all(
        sessions.map((s) => api.invoke('get_session', { sessionId: s.id }))
      );
      return statuses.every((s) => s.status !== 'running');
    },
    { timeout: 120_000, every: 1000, giveUp: () => appExited }
  ).catch((error) => {
    if (appExited) throw error;
    log(`warning: ${error.message}`);
    result.warnings.push(`${name}: ${error.message}`);
  });
  if (scenario.chat) {
    if (!params.skipVerify) {
      step('checking the chat against the store');
      checks.chat = await verifyChat({ api, frontend, session: viewed });
    }
    await frontend.evaluate(ui.closeChat());
  }

  const only = (tags) =>
    Object.fromEntries(Object.entries(agentsAfter).filter(([tag]) => tags.includes(tag)));
  const tags = sessions.map((s) => s.tag);
  const out = {
    about: scenario.about,
    seconds,
    host: { loadAverage1mStart: loadAverageStart, loadAverage1mEnd: loadAverageEnd },
    load: {
      ...agentLoad(agentsBefore, only(tags), seconds, expectedPerSecond),
      viewedSessions: viewedSessions.length,
      rows: mountedRows(rowsAtStart, rowsAtEnd),
      chatSession: scenario.chat ? viewed.tag : null,
      promptBytes: Buffer.byteLength(promptFor(viewed)),
      sessionRowBytes,
    },
    backend: backendResult({
      metrics,
      usage,
      seconds,
      frontendKind: frontend.kind,
      streamSupported,
    }),
    frontend: {
      ...frontendResult({ collected, usage, seconds, streamSupported }),
      subtitleStaleness,
      catchUpAfterShownMs: catchUpMs,
      shown,
    },
    checks,
  };
  warn(name, out);
  return out;
}

/** What a reader of a scenario's figures should know before trusting them. */
function warn(name, scenario) {
  const add = (text) => result.warnings.push(`${name}: ${text}`);
  const { rows, rateShare } = scenario.load;
  if (rows.notMounted.length) {
    add(
      `${rows.notMounted.length} of the viewed project's ${scenario.load.viewedSessions} rows were not mounted while it was measured (${rows.notMounted.slice(0, 8).join(', ')}${rows.notMounted.length > 8 ? ', …' : ''}); the page figures cover the ${rows.mounted} that were. A taller --viewport-height mounts more.`
    );
  }
  // Sessions started together make their tool calls together, so a short
  // window can hold a little more or less than its share of them.
  if (rateShare !== null && rateShare < 0.8)
    add(`the agents wrote ${(rateShare * 100).toFixed(0)}% of the rate asked for`);
  const lag = scenario.frontend.timerLagMs?.p50;
  if (frontend.kind === 'desktop' && lag > 100) {
    add(
      `the page's timers ran ${Math.round(lag)} ms late at the median (the desktop window is never on screen), so anything the page does on a timer was measured throttled`
    );
  }
  for (const [check, outcome] of Object.entries(scenario.checks)) {
    if (!outcome.ok) result.checks.failed.push(`${name}: ${check}`);
  }
  result.checks.ok = result.checks.failed.length === 0;
}

async function main() {
  const profiles = writeAgentConfig();
  const { api, dataDir, hooks } = await startApp();
  log(`app running, pid ${app.pid}, data in ${dataDir}`);
  result.hooks = hooks;
  if (hooks < HOOKS_WANTED) {
    result.warnings.push(
      `the app's harness hooks are version ${hooks} and this harness expects ${HOOKS_WANTED}: there is no subtitle staleness figure, tool output is counted with text in agentWriteToStoredMs, and the backend percentiles are coarse (half an octave)`
    );
  }
  writeResult(dataDir);
  // Commands this tree does not have answer "Unknown command".
  const streamSupported = await api.invoke('check_session_stream', {}).then(
    () => true,
    (error) => !/unknown command/i.test(error.message)
  );
  result.streamSupported = streamSupported;
  phase = 'seeding the projects';
  const projects = await seed({
    api,
    runDir,
    home,
    env,
    projects: params.projects,
    sessionsPerProject: params.sessionsPerProject,
    scenarios: params.scenarios,
  });

  phase = 'opening the frontend';
  if (params.frontend === 'browser') {
    // The app's own webview loaded the frontend too; park it, so the browser is the only client.
    await api.evalInWebviews(`location.replace('about:blank');`);
    frontend = await openBrowserFrontend({
      api,
      executable: config.browser,
      profileDir: join(runDir, 'browser'),
      env,
      logFile: join(runDir, 'logs', 'browser.log'),
      localStorage: params.localStorage,
      viewportHeight: params.viewportHeight,
    });
  } else {
    frontend = await openDesktopFrontend({ api, localStorage: params.localStorage });
  }
  sampler = startSampler({
    appPid: app.pid,
    browserPid: frontend.pid,
    runDir,
    realStaged: config.realStaged,
    allowedExecutables: config.browser ? [config.browser] : [],
  });

  for (const [index, name] of params.scenarios.entries()) {
    phase = `scenario ${name}`;
    result.notRun = params.scenarios.slice(index + 1);
    try {
      result.scenarios[name] = await runScenario({
        name,
        index,
        api,
        projects: [projects.viewed[index], ...projects.background],
        profiles,
        streamSupported,
      });
    } catch (error) {
      result.scenarios[name] = {
        about: SCENARIOS[name].about,
        failure: { phase, message: error.message, ...(error.detail && { detail: error.detail }) },
      };
      throw error;
    } finally {
      await sampler.sampleNow().catch(() => {});
      writeResult();
    }
  }
  result.complete = true;
  writeResult();
}

for (const name of ['SIGTERM', 'SIGINT', 'SIGHUP']) {
  process.on(name, () => {
    stop();
    process.exit(130);
  });
}
main().then(
  () => {
    stop();
    process.exit(0);
  },
  (error) => {
    log(`failed while ${phase}: ${error.stack ?? error}`);
    result.failure = {
      phase,
      message: error.message,
      ...(error.detail && { detail: error.detail }),
    };
    try {
      writeResult();
    } catch (writeError) {
      log(`could not write the result: ${writeError.message}`);
    }
    stop();
    process.exit(1);
  }
);
