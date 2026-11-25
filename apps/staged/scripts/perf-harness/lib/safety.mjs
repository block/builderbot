// What keeps a run away from the user's real Staged data, their agent tools
// and their credentials.
//
// The app keeps everything under `$HOME/.staged` and has no override, so the
// harness runs it (and everything else it starts) with HOME pointing into a
// throwaway directory, on a minimal PATH. This module decides whether that
// is really so, and the harness refuses to start when it is not.

import { existsSync, mkdirSync, mkdtempSync, realpathSync, statSync } from 'node:fs';
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

/** The PATH of everything a run starts: its own `bin` and the system directories. */
export const runPath = (runDir) =>
  [join(runDir, 'bin'), '/usr/bin', '/bin', '/usr/sbin', '/sbin'].join(':');

/**
 * Login files for the throwaway HOME, by name.
 *
 * The app looks a program up by asking a real login shell
 * (`/bin/zsh -l -c 'whence -p …'`, then `/bin/bash -l -c 'type -P …'`; see
 * `crates/doctor/src/resolve.rs` and `crates/blox-cli`). A login shell
 * throws away the PATH it was given and builds one from `/etc/paths`,
 * which can name the user's own directories, so the lookup finds the
 * user's tools whatever PATH the run has. The user's login files are read
 * after the system's, from HOME, and HOME is the run's: these put the
 * run's PATH back, so a lookup finds what the run itself would find.
 */
export function loginFiles(runDir) {
  const text = `# Staged load-test harness: a login shell under this HOME searches the run's PATH only.\nPATH='${runPath(runDir)}'\nexport PATH\n`;
  // zsh reads `.zprofile` and then `.zlogin`, after the system's `zprofile`
  // and `zlogin`; bash reads `.bash_profile`, and as `sh`, `.profile`.
  return { '.zprofile': text, '.zlogin': text, '.bash_profile': text, '.profile': text };
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
    PATH: runPath(runDir),
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

/**
 * Whether `path` is in a system directory. Such a path is taken as it was
 * started: those directories are root-owned, and a distribution may link a
 * tool there to its real file elsewhere (Ubuntu's `/bin/cat` is uutils'
 * `/usr/lib/cargo/bin/coreutils/cat`). Links are followed only from the
 * other expected places, which is where one could lead to a real agent.
 */
const isSystemPath = (path) => EXPECTED_EXECUTABLE_ROOTS.some((root) => path.startsWith(root));

/** Where a run's programs may be: its own directory, the system's, and any named besides. */
export const expectedPlaces = ({ runDir, allowed = [] }) => [
  runDir + '/',
  ...EXPECTED_EXECUTABLE_ROOTS,
  ...allowed,
];

/** `path` with its symlinks followed when it is a file, else `null`. */
export function realFile(path) {
  try {
    return statSync(path).isFile() ? realpathSync(path) : null;
  } catch {
    return null;
  }
}

/**
 * What to report when the file at `path` is not in one of the `expected`
 * places, or `null` when it is. A path inside the run directory that is a
 * link to a file outside it counts as outside, and both are named. A path in
 * a system directory is accepted as it is, wherever its links lead.
 */
export function outsidePlace(path, expected, real = realFile) {
  const inside = (candidate) => expected.some((root) => candidate.startsWith(root));
  if (!inside(path)) return path;
  if (isSystemPath(path)) return null;
  const resolved = real(path);
  return resolved && !inside(resolved) ? `${resolved} (through ${path})` : null;
}

/** Whether `path` is a file someone may run. */
export function isExecutableFile(path) {
  try {
    const stat = statSync(path);
    return stat.isFile() && (stat.mode & 0o111) !== 0;
  } catch {
    return false;
  }
}

/**
 * The program outside the `expected` places that a process's command line
 * (as `ps` prints it) runs, or `null`: the program itself, or the script
 * it was given. A real agent is as often a script as a binary
 * (`node /opt/homebrew/bin/codex-acp`), and one run by the run's own Node
 * or by `/bin/sh` has a program that is where it should be.
 */
export function outsideProgram(
  args,
  expected,
  { real = realFile, executable = isExecutableFile } = {}
) {
  // `ps` shows a process that has exited as `(name)`: nothing left to check.
  if (args.startsWith('(')) return null;
  const inside = (candidate) => expected.some((root) => candidate.startsWith(root));
  const words = args.split(' ');
  // A bare name was found through the run's PATH (the run's own `bin` and
  // the system directories); `lsof` then says which file it really is.
  if (words[0].includes('/')) {
    // Against the whole line, so a program whose path has a space in it
    // (an installed browser) is still recognized by its prefix.
    if (!inside(args)) return args.slice(0, 160);
    // In its place by name; a link there to a file elsewhere is not, unless
    // the place is a system directory.
    const resolved = isSystemPath(words[0]) ? null : real(words[0]);
    if (resolved && !inside(resolved)) return `${resolved} (through ${words[0]})`;
  }
  // The first word that is not an option is where a script goes. It counts
  // only when it is a file with an executable bit, as an installed script
  // has: a directory, a data file or a path that does not exist is an
  // argument.
  const script = words.slice(1).find((word) => !word.startsWith('-'));
  if (script?.startsWith('/') && executable(script)) {
    const outside = outsidePlace(script, expected, real);
    if (outside) return `${outside}, run by ${words[0]}`;
  }
  return null;
}
