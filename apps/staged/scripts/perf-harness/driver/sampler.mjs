// Once a second, from outside the app: the CPU time and memory of the app
// process, of what it started (the fake agents, git) and of the browser; and
// whether any of them, or the driver, has a file open under the real
// `~/.staged` or runs a program from somewhere unexpected.
//
// It starts with the app, before anything is seeded, and the browser is
// added when there is one. A process that starts and exits between two
// samples is not seen; `lib/spawns.mjs` reads the app's log for those.

import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

import { listProcesses, tree } from '../lib/processes.mjs';
import { expectedPlaces, outsideProgram } from '../lib/safety.mjs';

const run = promisify(execFile);
const LSOF_EVERY = 3;

/**
 * The CPU time of a set of processes that changes, as one running total:
 * call it with each sample's processes (`[{ pid, cpuSeconds }]`) and it
 * returns the total so far. Each process adds what it used since the sample
 * before; one seen for the first time adds all it has used. So a process
 * that exits keeps what it had added, where a sum over the processes alive
 * at each sample loses a process's whole lifetime when it exits (and the
 * difference of two such sums can be negative). What a process uses after
 * its last sample is not seen, nor is one that starts and exits between two
 * samples.
 */
export function cpuMeter() {
  let total = 0;
  let last = new Map();
  return (processes) => {
    const now = new Map();
    for (const { pid, cpuSeconds } of processes) {
      const before = last.get(pid);
      // Less than before is a new process under a reused pid (or one that
      // has exited, which `ps` shows with no time): it starts from nothing.
      total += before === undefined || cpuSeconds < before ? cpuSeconds : cpuSeconds - before;
      now.set(pid, cpuSeconds);
    }
    last = now;
    return total;
  };
}

export function startSampler({ appPid, runDir, realStaged, allowedExecutables }) {
  const samples = [];
  const openUnderRealStaged = new Map();
  const unexpectedExecutables = new Set();
  const expected = expectedPlaces({ runDir, allowed: allowedExecutables });
  let browserPid = null;
  let lsofSamples = 0;
  let tick = 0;
  let busy = false;

  const inExpectedPlace = (path) => expected.some((root) => path.startsWith(root));
  // One answer per command line: the same few are seen every second.
  const judged = new Map();
  const outside = (args) => {
    if (!judged.has(args)) judged.set(args, outsideProgram(args, expected));
    return judged.get(args);
  };

  const meters = { app: cpuMeter(), children: cpuMeter(), browser: cpuMeter() };
  const sum = (group, processes) => ({
    cpuSeconds: meters[group](processes),
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
        app: sum('app', app.slice(0, 1)),
        children: sum('children', app.slice(1)),
        browser: sum('browser', browser),
      });
      const watched = [...app, ...browser, ...tree(all, process.pid)];
      for (const p of watched) {
        const program = outside(p.args);
        if (program) unexpectedExecutables.add(program);
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
    /** The browser's process, once there is one: measured and checked from then on. */
    watchBrowser(pid) {
      browserPid = pid;
    },
    safety: () => ({
      processSamples: samples.length,
      openFileSamples: lsofSamples,
      openUnderRealStaged: [...openUnderRealStaged.keys()],
      unexpectedExecutables: [...unexpectedExecutables],
    }),
    stop: () => clearInterval(timer),
  };
}
