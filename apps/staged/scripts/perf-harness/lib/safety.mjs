// What keeps a run away from the user's real Staged data, their agent tools
// and their credentials.
//
// The app keeps everything under `$HOME/.staged` and has no override, so the
// harness runs it (and everything else it starts) with HOME pointing into a
// throwaway directory, on a minimal PATH. This module decides whether that
// is really so, and the harness refuses to start when it is not.

import { existsSync, mkdirSync, mkdtempSync, realpathSync } from 'node:fs';
import { tmpdir, userInfo } from 'node:os';
import { dirname, join, resolve, sep } from 'node:path';

/** The user's home as the account database has it, whatever `$HOME` says. */
export const realHome = () => userInfo().homedir;

/** A path with every existing part resolved; a part that does not exist yet is kept as written. */
export function resolveExisting(path) {
  let existing = resolve(path);
  const rest = [];
  while (!existsSync(existing)) {
    const parent = dirname(existing);
    if (parent === existing) break;
    rest.unshift(existing.slice(parent.length + (parent.endsWith(sep) ? 0 : 1)));
    existing = parent;
  }
  return join(realpathSync(existing), ...rest);
}

export const isInside = (child, parent) =>
  child === parent || child.startsWith(parent.endsWith(sep) ? parent : parent + sep);

/** The real `~/.staged`, resolved, so a symlinked data directory is recognized too. */
export const realStagedDir = (home = realHome()) => resolveExisting(join(home, '.staged'));

/**
 * Why a run with this layout must not start, or `null` when it may.
 *
 * `runDir` is the throwaway directory, `home` the HOME everything gets, and
 * `executables` the programs the harness itself will run from.
 */
export function isolationProblem({ runDir, home, executables = [], userHome = realHome() }) {
  const real = realStagedDir(userHome);
  const run = resolveExisting(runDir);
  const dataDir = resolveExisting(join(home, '.staged'));
  if (isInside(run, real))
    return `the run directory ${run} is inside the real data directory ${real}`;
  if (isInside(real, run))
    return `the run directory ${run} contains the real data directory ${real}`;
  if (isInside(resolveExisting(userHome), run))
    return `the run directory ${run} contains the real home`;
  if (!isInside(dataDir, run) || dataDir === run)
    return `the data directory ${dataDir} is not inside the run directory ${run}`;
  if (isInside(dataDir, real)) return `the data directory ${dataDir} is the real one`;
  for (const executable of executables) {
    const resolved = resolveExisting(executable);
    if (isInside(resolved, real))
      return `${resolved} is inside the real data directory ${real}; run the harness with a Node outside it`;
  }
  return null;
}

/** A fresh throwaway directory with the HOME inside it. */
export function createRunDir() {
  const runDir = mkdtempSync(join(realpathSync(tmpdir()), 'staged-perf-'));
  const home = join(runDir, 'home');
  mkdirSync(home);
  return { runDir, home };
}

/**
 * The whole environment of everything the harness starts: nothing inherited,
 * so no credentials, no agent configuration and no PATH entry of the user's
 * comes along.
 */
export function scrubbedEnv({ runDir, home }) {
  return {
    HOME: home,
    CFFIXED_USER_HOME: home,
    TMPDIR: join(runDir, 'tmp'),
    PATH: [join(runDir, 'bin'), '/usr/bin', '/bin', '/usr/sbin', '/sbin'].join(':'),
    // The app captures a login shell's environment; this one is `/bin/sh`
    // without the login files, so the PATH above is what it reports.
    SHELL: join(runDir, 'bin', 'perf-shell'),
    USER: 'stagedperf',
    LOGNAME: 'stagedperf',
    LANG: 'en_US.UTF-8',
    GOOSE_DISABLE_KEYRING: '1',
    GIT_CONFIG_NOSYSTEM: '1',
  };
}

/** Executables outside these are reported by a run as unexpected. */
export const EXPECTED_EXECUTABLE_ROOTS = [
  '/usr/bin/',
  '/bin/',
  '/usr/sbin/',
  '/sbin/',
  '/usr/libexec/',
  '/System/',
  '/Library/Developer/',
  '/Applications/Xcode',
];
