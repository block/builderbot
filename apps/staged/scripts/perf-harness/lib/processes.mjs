// Finding and ending the processes of a run. Everything a run starts lives
// in its throwaway directory (the app, the fake agents, the staged Node) or
// has it in its arguments (the browser's profile) or as its working
// directory (git's fsmonitor daemon in a worktree), so a run's processes can
// be found by that directory alone, even ones that left the process group.

import { execFileSync } from 'node:child_process';

const sh = (file, args) => {
  try {
    return execFileSync(file, args, {
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'ignore'],
    });
  } catch (error) {
    // lsof exits non-zero when some process could not be read; what it printed still counts.
    return typeof error.stdout === 'string' ? error.stdout : '';
  }
};

/** Every process: `{ pid, ppid, rssKb, cpuSeconds, args }`. */
export function listProcesses() {
  const out = [];
  for (const line of sh('/bin/ps', ['-axo', 'pid=,ppid=,rss=,cputime=,args=']).split('\n')) {
    const m = line.match(/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s+(.*)$/);
    if (m)
      out.push({ pid: +m[1], ppid: +m[2], rssKb: +m[3], cpuSeconds: cpuSeconds(m[4]), args: m[5] });
  }
  return out;
}

/** `ps` prints cputime as `[[dd-]hh:]mm:ss.cc`. */
export function cpuSeconds(text) {
  const [days, rest] = text.includes('-') ? text.split('-') : ['0', text];
  return (
    rest.split(':').reduce((total, part) => total * 60 + Number(part), 0) + Number(days) * 86400
  );
}

/** `root` and everything below it, from one `listProcesses()` result. */
export function tree(processes, root) {
  const children = new Map();
  for (const p of processes) children.set(p.ppid, [...(children.get(p.ppid) ?? []), p]);
  const out = [];
  const walk = (pid) => {
    for (const child of children.get(pid) ?? []) {
      out.push(child);
      walk(child.pid);
    }
  };
  const self = processes.find((p) => p.pid === root);
  if (self) out.push(self);
  walk(root);
  return out;
}

/** Pids whose arguments name `dir` or whose working directory is inside it. */
export function processesOf(dir) {
  const found = new Map();
  for (const p of listProcesses()) {
    if (p.pid !== process.pid && p.args.includes(dir)) found.set(p.pid, p.args);
  }
  let pid = null;
  for (const line of sh('/usr/sbin/lsof', ['-a', '-d', 'cwd', '-Fpn']).split('\n')) {
    if (line.startsWith('p')) pid = Number(line.slice(1));
    else if (
      line.startsWith('n') &&
      pid &&
      pid !== process.pid &&
      (line.slice(1) === dir || line.slice(1).startsWith(dir + '/'))
    ) {
      if (!found.has(pid)) found.set(pid, `(cwd ${line.slice(1)})`);
    }
  }
  return found;
}

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};
const signal = (pid, name) => {
  try {
    process.kill(pid, name);
  } catch {
    /* already gone */
  }
};
const pause = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);

/**
 * End every process of the run in `dir`: the process group first, then
 * whatever still names the directory. Synchronous, so it can run from an
 * exit or signal handler. Returns what survived, which should be nothing.
 */
export function endRun(dir, group) {
  if (group) signal(-group, 'SIGTERM');
  for (let round = 0; round < 5; round++) {
    const found = processesOf(dir);
    if (group && alive(group)) found.set(group, '(run group leader)');
    if (found.size === 0) return [];
    for (const pid of found.keys()) signal(pid, round < 2 ? 'SIGTERM' : 'SIGKILL');
    if (group && round >= 2) signal(-group, 'SIGKILL');
    pause(round < 2 ? 1500 : 500);
  }
  return [...processesOf(dir)].map(([pid, args]) => ({ pid, args }));
}
