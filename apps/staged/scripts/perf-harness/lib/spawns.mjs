// What the app's log says it started.
//
// The app logs one line for each agent it starts through the login shell,
// which is how action detection starts one when a project is created:
//
//   … Spawning Goose through login shell /…/perf-shell (exec line on stdin); exec line: exec '/…/goose' 'acp' …
//
// The line is written before the start, so it is there however short the
// process lives; a process sampler misses one that is gone in 50 ms. Only
// a debug build logs (the log plugin is installed under `debug_assertions`),
// and an agent a session starts directly, without the shell, is not logged.

import { closeSync, openSync, readFileSync, readSync } from 'node:fs';
import { join } from 'node:path';

/** The line as the app formats it (`crates/acp-client/src/driver.rs`); a test holds the two together. */
export const SPAWN_LOG_FORMAT = 'Spawning {} through login shell {} ({}); exec line: {}';
const SPAWN_LINE = /Spawning (.+?) through login shell (\S+) \(([^)]*)\); exec line: (.*)$/;

/**
 * The starts in a piece of log: `[{ label, shell, paths }]`, where `paths`
 * are the absolute paths the exec line names: the agent, the interpreter
 * it is run with when the app names one, and the directory the app adds to
 * the PATH for it. Every one of them is a place the start reaches into.
 */
export function spawnsIn(logText) {
  const out = [];
  for (const line of logText.split('\n')) {
    const match = line.match(SPAWN_LINE);
    if (!match) continue;
    const [, label, shell, , execLine] = match;
    // The app single-quotes each word, an inner quote as `'\''`.
    const words = [...execLine.matchAll(/'((?:[^']|'\\'')*)'/g)].map(([, word]) =>
      word.replaceAll(`'\\''`, `'`)
    );
    out.push({ label, shell, paths: words.filter((word) => word.startsWith('/')) });
  }
  return out;
}

/**
 * Reads a log file as it grows: each `read()` returns the starts in the
 * whole lines written since the one before. A file that does not exist
 * yet has none.
 */
export function followSpawns(logFile) {
  let offset = 0;
  // The bytes after the last whole line, kept as bytes so a character cut
  // in two by a read is put together again.
  let partial = Buffer.alloc(0);
  return {
    read() {
      let fd;
      try {
        fd = openSync(logFile, 'r');
      } catch {
        return [];
      }
      const chunks = [partial];
      try {
        const buffer = Buffer.alloc(1 << 16);
        for (;;) {
          const n = readSync(fd, buffer, 0, buffer.length, offset);
          if (n === 0) break;
          chunks.push(Buffer.from(buffer.subarray(0, n)));
          offset += n;
        }
      } finally {
        closeSync(fd);
      }
      const bytes = Buffer.concat(chunks);
      const end = bytes.lastIndexOf(0x0a) + 1;
      partial = bytes.subarray(end);
      return spawnsIn(bytes.subarray(0, end).toString('utf8'));
    },
  };
}

/**
 * Why the app built from the tree at `appDir` will not log the agents it
 * starts, or `null` when it will.
 */
export function spawnLogProblem({ appDir, profile }) {
  if (profile !== 'debug') return `a ${profile} build of the app writes no log`;
  let source = '';
  try {
    source = readFileSync(
      join(appDir, '..', '..', 'crates', 'acp-client', 'src', 'driver.rs'),
      'utf8'
    );
  } catch {
    /* reported below */
  }
  return source.includes(SPAWN_LOG_FORMAT)
    ? null
    : 'this tree does not log an agent start in the form the harness reads';
}
