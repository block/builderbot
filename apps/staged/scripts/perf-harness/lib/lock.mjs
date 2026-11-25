// A lock between harness builds, for a directory more than one of them
// writes: the cargo target directory, which every tree's harness build
// shares by default. Without it, a build of another tree can replace the
// binary between this run's `cargo build` and its copy of the result, and
// the run then measures a binary it did not build.
//
// The lock is a directory holding its owner (`owner.json`: the pid, an id
// of this taking and when it was taken). Taking it is one rename of a
// directory prepared beside it, which fails while the lock exists: a
// directory cannot be renamed onto one that is not empty, and a lock always
// holds its owner file. Releasing it is one rename too, away from the
// lock's name. So there is no moment at which the lock exists without an
// owner, in the taking or in the letting go.

import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import {
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, join } from 'node:path';

const OWNER = 'owner.json';
/** A buried lock is kept this long; see `bury`. */
const BURIED_FOR_MS = 60 * 60 * 1000;
/** How long `withLock` waits for a holder unless it is told otherwise. */
export const LOCK_WAIT_MS = 60 * 60 * 1000;
/** The `code` of the error `withLock` gives up with. */
export const LOCK_WAIT_EXPIRED = 'ELOCKWAIT';

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // Not ours to signal, but there.
    return error.code === 'EPERM';
  }
};
// A timer, not a blocking wait: the process goes on answering its signals
// while it waits, so an interrupt ends a wait as it ends anything else.
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const ownerOf = (lock) => {
  try {
    return JSON.parse(readFileSync(join(lock, OWNER), 'utf8'));
  } catch {
    return null;
  }
};

/**
 * Move aside a lock whose owner has died. The name it is moved to is made
 * from that taking's id, so everyone who saw the same dead owner aims at
 * the same name, and it stays taken for an hour: a second attempt, made by
 * someone still acting on the dead owner after the lock was taken afresh,
 * fails on it and leaves the new lock alone.
 */
function bury(lock, owner) {
  const grave = `${lock}.dead-${owner.id}`;
  try {
    renameSync(lock, grave);
    // Its age is counted from now, not from when the lock was taken.
    utimesSync(grave, new Date(), new Date());
  } catch {
    /* buried by another waiter, or released */
  }
}

/** Remove buried locks past their hour, and what a release could not remove. */
function clearBuried(lock) {
  const dir = dirname(lock);
  const prefixes = [`${basename(lock)}.dead-`, `${basename(lock)}.released-`];
  for (const name of readdirSync(dir)) {
    if (!prefixes.some((prefix) => name.startsWith(prefix))) continue;
    try {
      if (Date.now() - statSync(join(dir, name)).mtimeMs > BURIED_FOR_MS)
        rmSync(join(dir, name), { recursive: true, force: true });
    } catch {
      /* removed by another build */
    }
  }
}

/**
 * Take `lock` if it is free. Returns `{ release }` when taken, and
 * `{ heldBy }` (the holder's pid, `null` if it let go meanwhile) when not.
 * A lock whose holder is no longer running is taken over.
 */
export function tryLock(lock, pid = process.pid) {
  const id = randomBytes(8).toString('hex');
  const mine = `${lock}.taking-${id}`;
  for (let attempt = 0; attempt < 2; attempt++) {
    mkdirSync(mine, { recursive: true });
    writeFileSync(join(mine, OWNER), JSON.stringify({ pid, id, since: new Date().toISOString() }));
    try {
      renameSync(mine, lock);
    } catch (error) {
      rmSync(mine, { recursive: true, force: true });
      if (!['ENOTEMPTY', 'EEXIST'].includes(error.code)) throw error;
      const owner = ownerOf(lock);
      if (!owner) return { heldBy: null };
      if (alive(owner.pid)) return { heldBy: owner.pid };
      bury(lock, owner);
      continue;
    }
    clearBuried(lock);
    return {
      release() {
        if (ownerOf(lock)?.id !== id) return;
        // Moved away whole, then removed. Removed in place it would be
        // emptied first, and an empty directory is one a waiter's rename
        // succeeds onto: the rest of the removal then meets the waiter's
        // lock, and fails on it or deletes it.
        const gone = `${lock}.released-${id}`;
        try {
          renameSync(lock, gone);
        } catch (error) {
          if (error.code === 'ENOENT') return;
          throw error;
        }
        try {
          rmSync(gone, { recursive: true, force: true });
        } catch {
          /* the lock is free; `clearBuried` removes what is left */
        }
      },
    };
  }
  return { heldBy: ownerOf(lock)?.pid ?? null };
}

/** What `ps` shows a process was started as, or `null` when it does not say. */
function commandOf(pid) {
  const shown = spawnSync('ps', ['-o', 'command=', '-p', String(pid)], { encoding: 'utf8' });
  return shown.status === 0 ? shown.stdout.trim() || null : null;
}

/** The holder of `lock` in words, for a message a person acts on. */
function describeHolder(lock) {
  const owner = ownerOf(lock);
  if (!owner) return `a process that could not be read from ${join(lock, OWNER)}`;
  const command = commandOf(owner.pid);
  return (
    `process ${owner.pid}` +
    (command ? ` (\`${command}\`)` : '') +
    (owner.since ? `, which took it at ${owner.since}` : '')
  );
}

/**
 * Run `work` holding `lock`, and resolve to what it returns. While another
 * running process holds the lock this waits, for at most `timeout` ms, and
 * then rejects with an error that names the holder (`code`:
 * `LOCK_WAIT_EXPIRED`); `work` has not run. The bound is what ends a wait
 * on a lock nobody will release: its holder died and something else now
 * runs under its process id, so it reads as held by a running process.
 *
 * The wait leaves the event loop running, so the process's signal handlers
 * run during it. `say` is told once, and again every half minute, whom it
 * is waiting for, and if the lock could not be released afterwards (which
 * does not change the outcome of `work`).
 */
export async function withLock(
  lock,
  work,
  { say = () => {}, every = 1000, timeout = LOCK_WAIT_MS } = {}
) {
  const started = Date.now();
  let saidAt = -Infinity;
  for (;;) {
    const taken = tryLock(lock);
    if (taken.release) {
      try {
        return await work();
      } finally {
        try {
          taken.release();
        } catch (error) {
          // The work's outcome stands. The lock goes with this process.
          say(
            `could not release ${lock} (${error.message}); it stays held until this process exits`
          );
        }
      }
    }
    const waited = Date.now() - started;
    if (waited >= timeout) {
      const error = new Error(
        `gave up after ${Math.round(waited / 1000)}s waiting for ${lock}, held by ${describeHolder(lock)}. ` +
          `If that process is not a harness build (the process id of a build that was killed can since have gone to another process), remove ${lock} and run again.`
      );
      error.code = LOCK_WAIT_EXPIRED;
      throw error;
    }
    if (taken.heldBy !== null && Date.now() - saidAt >= 30_000) {
      saidAt = Date.now();
      say(`waiting for the harness build of process ${taken.heldBy}, which holds ${lock}`);
    }
    await pause(Math.min(every, timeout - waited));
  }
}
