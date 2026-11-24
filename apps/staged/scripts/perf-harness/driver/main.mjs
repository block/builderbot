// The driver: runs from inside the throwaway directory with the scrubbed
// environment `run.mjs` gave it. Starts the app, seeds it, opens the
// frontend, runs the scenarios and writes `out/result.json`.
//
//   node main.mjs <config.json>

import { mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { loadavg } from 'node:os';
import { join } from 'node:path';

import { isInside, resolveExisting } from '../lib/safety.mjs';
import { createClient, launchApp } from './app.mjs';
import { openBrowserFrontend, openDesktopFrontend } from './frontend.mjs';
import { agentLoad, backendResult, frontendResult } from './measure.mjs';
import { startSampler } from './sampler.mjs';
import { plan, SCENARIOS } from './scenarios.mjs';
import { seed } from './seed.mjs';
import * as ui from './ui.mjs';
import { log, sleep, until } from './util.mjs';

const config = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const { runDir, home, params } = config;
const env = process.env;
let app;
let frontend;
let sampler;

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
    // A turn outlasts its scenario; the driver cancels it when the window closes.
    turnSeconds: params.warmup + params.duration + 300,
  });
  writeFileSync(
    config.agentConfig,
    JSON.stringify({ profiles: { background: shape(params.rate), chat: shape(params.chatRate) } })
  );
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
  let exited = null;
  app.once(
    'exit',
    (code, signal) => (exited = `the app exited (${signal ?? code}); see logs/app.log`)
  );
  const api = createClient({ port: config.port, token: config.token });
  const metrics = await until(
    'the app to serve its harness port',
    () => {
      if (exited) throw new Error(exited);
      return api.metrics();
    },
    { timeout: 180_000, every: 500 }
  ).catch((error) => {
    throw new Error(exited ?? error.message);
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
  });
  return { api, dataDir };
}

async function runScenario({ name, index, api, projects, streamSupported }) {
  const scenario = SCENARIOS[name];
  const sessions = plan({ name, index, projects, running: params.running });
  log(`scenario ${name}: starting ${sessions.length} sessions`);
  for (const session of sessions) {
    const started = await api.invoke('start_or_queue_branch_session', {
      branchId: session.branchId,
      prompt: `Load test pf-tag=${session.tag} pf-profile=${session.profile}`,
      sessionType: 'note',
      provider: 'claude',
      imageIds: null,
      launchContext: null,
      acpConfigSelection: null,
    });
    session.id = started.sessionId;
  }
  await until(
    'every session to be streaming',
    () => {
      const state = agentState();
      return sessions.every((s) => state[s.tag]?.chunks > 0);
    },
    { timeout: 240_000, every: 500 }
  );

  const viewed = sessions.find((s) => s.project === 0);
  await frontend.reload();
  await frontend.evaluate(ui.openProject(projects[0].name, viewed.tag));
  if (scenario.chat) await frontend.evaluate(ui.openChat(viewed.tag));
  await sleep(params.warmup * 1000);
  if (scenario.hidden) {
    await frontend.evaluate(ui.setHidden(true));
    await sleep(1000);
  }

  log(`scenario ${name}: measuring for ${params.duration}s`);
  await api.resetMetrics();
  await frontend.reset();
  const agentsBefore = agentState();
  const from = Date.now();
  await sleep(params.duration * 1000);
  const to = Date.now();
  const seconds = (to - from) / 1000;
  const [metrics, collected] = await Promise.all([api.metrics(), frontend.collect()]);
  const agentsAfter = agentState();
  await sampler.sampleNow();
  const usage = sampler.between(from, to);
  const shown = await frontend.evaluate(ui.describe());

  let catchUpMs = null;
  if (scenario.hidden) {
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

  if (scenario.chat) await frontend.evaluate(ui.closeChat());
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
    { timeout: 120_000, every: 1000 }
  ).catch((error) => log(`warning: ${error.message}`));

  const only = (tags) =>
    Object.fromEntries(Object.entries(agentsAfter).filter(([tag]) => tags.includes(tag)));
  const tags = sessions.map((s) => s.tag);
  return {
    about: scenario.about,
    seconds,
    load: {
      ...agentLoad(agentsBefore, only(tags), seconds),
      viewedSessions: sessions.filter((s) => s.project === 0).length,
      chatSession: scenario.chat ? viewed.tag : null,
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
      catchUpAfterShownMs: catchUpMs,
      shown,
    },
  };
}

async function main() {
  writeAgentConfig();
  const loadStart = loadavg();
  const { api, dataDir } = await startApp();
  log(`app running, pid ${app.pid}, data in ${dataDir}`);
  // Commands this tree does not have answer "Unknown command".
  const streamSupported = await api.invoke('check_session_stream', {}).then(
    () => true,
    (error) => !/unknown command/i.test(error.message)
  );
  const projects = await seed({
    api,
    runDir,
    home,
    env,
    projects: params.projects,
    sessionsPerProject: params.sessionsPerProject,
  });

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

  const scenarios = {};
  for (const [index, name] of params.scenarios.entries()) {
    scenarios[name] = await runScenario({ name, index, api, projects, streamSupported });
  }
  await sampler.sampleNow();
  const result = {
    schema: 1,
    label: params.label,
    startedAt: config.startedAt,
    tree: config.tree,
    frontend: frontend.kind,
    streamSupported,
    params,
    host: { platform: process.platform, loadAverageStart: loadStart, loadAverageEnd: loadavg() },
    scenarios,
    safety: { runDir, dataDir, realStagedDir: config.realStaged, ...sampler.safety() },
  };
  writeFileSync(join(runDir, 'out', 'result.json'), JSON.stringify(result, null, 2));
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
    log(`failed: ${error.stack ?? error}`);
    stop();
    process.exit(1);
  }
);
