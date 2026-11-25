// A lock between harness builds, for a directory more than one of them
// writes: the cargo target directory, which every tree's harness build
// shares by default. Without it, a build of another tree can replace the
// binary between this run's `cargo build` and its copy of the result, and
// the run then measures a binary it did not build.
//
// The lock is a directory holding its owner (`owner.json`: the pid and an
// id of this taking). Taking it is one rename of a directory prepared
// beside it, which fails while the lock exists: a directory cannot be
// renamed onto one that is not empty, and a lock always holds its owner
// file. So there is no moment at which the lock exists without an owner.

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

const alive = (pid) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    // Not ours to signal, but there.
    return error.code === 'EPERM';
  }
};
const pause = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
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

function clearBuried(lock) {
  const dir = dirname(lock);
  const prefix = `${basename(lock)}.dead-`;
  for (const name of readdirSync(dir)) {
    if (!name.startsWith(prefix)) continue;
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
    writeFileSync(join(mine, OWNER), JSON.stringify({ pid, id }));
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
        if (ownerOf(lock)?.id === id) rmSync(lock, { recursive: true, force: true });
      },
    };
  }
  return { heldBy: ownerOf(lock)?.pid ?? null };
}

/**
 * Run `work` holding `lock`, waiting for as long as another running
 * process holds it. `say` is told once, and again every half minute, whom
 * it is waiting for.
 */
export function withLock(lock, work, { say = () => {}, every = 1000 } = {}) {
  let waited = 0;
  for (;;) {
    const taken = tryLock(lock);
    if (taken.release) {
      try {
        return work();
      } finally {
        taken.release();
      }
    }
    if (waited % 30_000 < every && taken.heldBy !== null)
      say(`waiting for the harness build of process ${taken.heldBy}, which holds ${lock}`);
    pause(every);
    waited += every;
  }
}
