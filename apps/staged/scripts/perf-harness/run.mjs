#!/usr/bin/env node
// Staged load-test harness: the entry point. See README.md.
//
//   node run.mjs [options]                       run the scenarios, write a result file
//   node run.mjs compare <a.json> <b.json> [--all]   print the difference between two results
//   node run.mjs --help
//
// A run builds the tree with the `perf-harness` cargo feature, copies what it
// needs into a throwaway directory, and hands over to driver/main.mjs there,
// under a HOME inside that directory and a minimal PATH. This file never
// starts the app itself; it checks the isolation first, and ends every
// process of the run when the driver stops, however it stops.

import { randomBytes } from 'node:crypto';
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { spawn } from 'node:child_process';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { build, describeTree } from './lib/build.mjs';
import { parseParams, usage } from './lib/params.mjs';
import { endRun } from './lib/processes.mjs';
import { compare, summarize } from './lib/report.mjs';
import {
  createRunDir,
  isolationProblem,
  realHome,
  realStagedDir,
  scrubbedEnv,
} from './lib/safety.mjs';
import { stage } from './lib/stage.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const argv = process.argv.slice(2);
const say = (...parts) => console.error('[perf-harness]', ...parts);

if (argv[0] === '--help' || argv[0] === '-h') {
  console.log(usage());
  process.exit(0);
}
if (argv[0] === 'compare') {
  const files = argv.slice(1).filter((arg) => !arg.startsWith('--'));
  if (files.length !== 2) {
    console.error(usage());
    process.exit(2);
  }
  const [a, b] = files.map((file) => JSON.parse(readFileSync(file, 'utf8')));
  console.log(compare(a, b, { all: argv.includes('--all') }));
  process.exit(0);
}

/** A Chromium to drive: the one named, else the newest Playwright headless shell, else an installed Chrome. */
function findBrowser(named) {
  if (named) return existsSync(named) ? named : null;
  const cache = join(realHome(), 'Library', 'Caches', 'ms-playwright');
  const shells = existsSync(cache)
    ? readdirSync(cache)
        .filter((name) => name.startsWith('chromium_headless_shell-'))
        .sort((x, y) => Number(y.split('-')[1]) - Number(x.split('-')[1]))
    : [];
  for (const shell of shells) {
    for (const arch of ['mac-arm64', 'mac-x64', 'mac']) {
      const path = join(cache, shell, `chrome-headless-shell-${arch}`, 'chrome-headless-shell');
      if (existsSync(path)) return path;
    }
  }
  return (
    [
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
      '/Applications/Chromium.app/Contents/MacOS/Chromium',
    ].find(existsSync) ?? null
  );
}

let params;
try {
  params = parseParams(argv[0] === 'run' ? argv.slice(1) : argv);
} catch (error) {
  console.error(`${error.message}\n\n${usage()}`);
  process.exit(2);
}
if (Number(process.versions.node.split('.')[0]) < 22) {
  say(`Node 22 or newer is needed (this is ${process.versions.node})`);
  process.exit(2);
}

const appDir = resolve(params.appDir || join(here, '..', '..'));
const outDir = resolve(
  params.out || join(realpathSync(tmpdir()), 'staged-perf-harness', 'results')
);
const { runDir, home } = createRunDir();
let driver = null;
let finished = false;

/** End the run's processes, keep what is worth keeping, remove the rest. Safe to call from any exit path. */
function finish(code, why) {
  if (finished) return;
  finished = true;
  if (why) say(why);
  const leftover = endRun(runDir, driver?.pid);
  const stamp = new Date().toISOString().replace(/[-:]/g, '').slice(0, 15);
  const base = join(outDir, `${params.label}-${stamp}`);
  mkdirSync(outDir, { recursive: true });
  const resultFile = join(runDir, 'out', 'result.json');
  let failed = code !== 0;
  if (existsSync(resultFile)) {
    // The driver writes this file as it goes, so a run that failed or was
    // stopped still has every scenario it finished.
    const result = JSON.parse(readFileSync(resultFile, 'utf8'));
    result.safety.leftoverProcesses = leftover;
    if (!result.complete && !result.failure)
      result.failure = { phase: 'running', message: why ?? 'the driver stopped' };
    writeFileSync(`${base}.json`, JSON.stringify(result, null, 2));
    writeFileSync(`${base}.md`, summarize(result));
    console.log(summarize(result));
    say(`result: ${base}.json`);
    say(`summary: ${base}.md`);
    if (result.failure) {
      const measured = Object.values(result.scenarios).filter((s) => !s.failure).length;
      say(
        `FAILED; the result holds the ${measured} scenario${measured === 1 ? '' : 's'} measured before the failure`
      );
      failed = true;
    }
    if (result.checks && !result.checks.ok) {
      say(`FAILED a shown-against-stored check: ${result.checks.failed.join(', ')}`);
      failed = true;
    }
    if (
      result.safety.openUnderRealStaged.length ||
      result.safety.unexpectedExecutables.length ||
      leftover.length
    ) {
      say('FAILED a safety check; see the Safety section above');
      failed = true;
    }
  } else if (leftover.length) {
    say(`processes left behind: ${leftover.map((p) => `${p.pid} ${p.args}`).join('; ')}`);
  }
  if (failed && existsSync(join(runDir, 'logs'))) {
    cpSync(join(runDir, 'logs'), `${base}-logs`, { recursive: true });
    say(`logs: ${base}-logs`);
  }
  if (params.keep) say(`kept the run directory: ${runDir}`);
  else rmSync(runDir, { recursive: true, force: true });
  process.exit(failed ? code || 3 : 0);
}
for (const name of ['SIGINT', 'SIGTERM', 'SIGHUP'])
  process.on(name, () => finish(130, `interrupted (${name})`));
process.on('uncaughtException', (error) => finish(1, `failed: ${error.stack ?? error}`));

// Nothing is built or started unless everything will stay inside the run directory.
const problem = isolationProblem({ runDir, home, executables: [process.execPath] });
if (problem) finish(2, `refusing to start: ${problem}`);
const browser =
  params.frontend === 'browser' ? findBrowser(params.browser || process.env.BROWSER_PATH) : null;
if (params.frontend === 'browser' && !browser) {
  finish(
    2,
    'no Chromium found: set BROWSER_PATH or pass --browser (a Playwright chromium_headless_shell or Google Chrome), or use --frontend desktop'
  );
}

say(`run directory: ${runDir}`);
let built;
try {
  // Awaited for the wait on another run's build, during which the signal
  // handlers above run.
  built = await build({
    appDir,
    profile: params.profile,
    port: params.port,
    skipBuild: params.skipBuild,
    targetDir: params.targetDir,
    buildWait: params.buildWait,
  });
} catch (error) {
  finish(2, `the build failed: ${error.message}`);
}
const staged = stage({ runDir, built, profile: params.profile });
const configFile = join(runDir, 'config.json');
writeFileSync(
  configFile,
  JSON.stringify({
    runDir,
    home,
    params,
    port: params.port,
    token: randomBytes(24).toString('hex'),
    binary: staged.binary,
    dist: staged.dist,
    agentConfig: staged.agentConfig,
    browser,
    realStaged: realStagedDir(),
    tree: describeTree(appDir),
    startedAt: new Date().toISOString(),
    parentPid: process.pid,
  })
);
// Its own process group, so one signal reaches everything it starts.
driver = spawn(staged.node, [staged.driver, configFile], {
  cwd: runDir,
  env: scrubbedEnv({ runDir, home }),
  detached: true,
  stdio: ['ignore', 'inherit', 'inherit'],
});
driver.on('exit', (code, signal) =>
  finish(code ?? 1, code === 0 ? null : `the driver stopped (${signal ?? `exit ${code}`})`)
);
