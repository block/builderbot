// Once a second, from outside the app: the CPU time and memory of the app
// process, of what it started (the fake agents, git) and of the browser; and
// whether any of them, or the driver, has a file open under the real
// `~/.staged` or runs a program from somewhere unexpected.

import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

import { listProcesses, tree } from '../lib/processes.mjs';
import { EXPECTED_EXECUTABLE_ROOTS } from '../lib/safety.mjs';

const run = promisify(execFile);
const LSOF_EVERY = 3;

export function startSampler({ appPid, browserPid, runDir, realStaged, allowedExecutables }) {
  const samples = [];
  const openUnderRealStaged = new Map();
  const unexpectedExecutables = new Set();
  const expected = [runDir + '/', ...EXPECTED_EXECUTABLE_ROOTS, ...allowedExecutables];
  let lsofSamples = 0;
  let tick = 0;
  let busy = false;

  const inExpectedPlace = (path) => expected.some((root) => path.startsWith(root));
  // `ps` shows a process that has exited as `(name)`: nothing left to check.
  // A bare name was found through the run's PATH (the run's own `bin` and
  // the system directories); `lsof` then says which file it really is.
  const fromExpectedPlace = (args) =>
    args.startsWith('(') || !args.split(' ')[0].includes('/') || inExpectedPlace(args);

  const sum = (processes) => ({
    cpuSeconds: processes.reduce((total, p) => total + p.cpuSeconds, 0),
    rssKb: processes.reduce((total, p) => total + p.rssKb, 0),
    processes: processes.length,
  });

  async function sample() {
    if (busy) return;
    busy = true;
    try {
      const all = listProcesses();
      const app = tree(all, appPid);
      const browser = browserPid ? tree(all, browserPid) : [];
      samples.push({
        t: Date.now(),
        app: sum(app.slice(0, 1)),
        children: sum(app.slice(1)),
        browser: sum(browser),
      });
      const watched = [...app, ...browser, ...tree(all, process.pid)];
      for (const p of watched) {
        if (!fromExpectedPlace(p.args)) unexpectedExecutables.add(p.args.slice(0, 160));
      }
      if (tick++ % LSOF_EVERY === 0) {
        const pids = [...new Set(watched.map((p) => p.pid))].join(',');
        const { stdout } = await run('/usr/sbin/lsof', ['-p', pids, '-Ffn'], {
          maxBuffer: 64 * 1024 * 1024,
        }).catch((error) => ({ stdout: error.stdout ?? '' }));
        lsofSamples += 1;
        let who = null;
        let kind = '';
        for (const line of stdout.split('\n')) {
          const value = line.slice(1);
          if (line.startsWith('p')) who = watched.find((p) => p.pid === Number(value));
          else if (line.startsWith('f')) kind = value;
          else if (line.startsWith('n') && who) {
            if (value === realStaged || value.startsWith(realStaged + '/'))
              openUnderRealStaged.set(`${value} (${who.args.slice(0, 80)})`, Date.now());
            // The program a process runs is mapped as text under its own name.
            const program = who.args.split(' ')[0].split('/').pop();
            if (kind === 'txt' && value.split('/').pop() === program && !inExpectedPlace(value))
              unexpectedExecutables.add(value);
          }
        }
      }
    } finally {
      busy = false;
    }
  }
  const timer = setInterval(() => void sample().catch(() => {}), 1000);
  void sample();

  return {
    /** The samples taken between two times, per group. */
    between(from, to) {
      const window = samples.filter((s) => s.t >= from - 500 && s.t <= to + 500);
      return Object.fromEntries(
        ['app', 'children', 'browser'].map((group) => [
          group,
          window.map((s) => ({ t: s.t, ...s[group] })),
        ])
      );
    },
    sampleNow: sample,
    safety: () => ({
      processSamples: samples.length,
      openFileSamples: lsofSamples,
      openUnderRealStaged: [...openUnderRealStaged.keys()],
      unexpectedExecutables: [...unexpectedExecutables],
    }),
    stop: () => clearInterval(timer),
  };
}
