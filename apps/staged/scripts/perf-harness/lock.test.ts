import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import {
  existsSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  unlinkSync,
  utimesSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The lock between harness builds (`lib/lock.mjs`). Its file system calls go
// through hooks here, so a test can stand where another process would: at
// one step of a release, or in the way of one.
const fsHooks = vi.hoisted(() => ({
  /** Called with the path of every `rmSync`, before it runs. */
  beforeRemove: null as null | ((path: string) => void),
  /** Called with both paths of every `renameSync`, before it runs. May throw. */
  beforeRename: null as null | ((from: string, to: string) => void),
  /** Called with both paths of every `renameSync` that succeeded. */
  afterRename: null as null | ((from: string, to: string) => void),
  /** Called with the path of every `writeFileSync`, before it runs. May throw. */
  beforeWrite: null as null | ((path: string) => void),
}));
vi.mock('node:fs', async (importOriginal) => {
  const actual = await importOriginal<typeof import('node:fs')>();
  return {
    ...actual,
    rmSync: (...args: Parameters<typeof actual.rmSync>) => {
      fsHooks.beforeRemove?.(String(args[0]));
      return actual.rmSync(...args);
    },
    renameSync: (...args: Parameters<typeof actual.renameSync>) => {
      fsHooks.beforeRename?.(String(args[0]), String(args[1]));
      actual.renameSync(...args);
      fsHooks.afterRename?.(String(args[0]), String(args[1]));
    },
    writeFileSync: (...args: Parameters<typeof actual.writeFileSync>) => {
      fsHooks.beforeWrite?.(String(args[0]));
      return actual.writeFileSync(...args);
    },
  };
});

import { LOCK_WAIT_EXPIRED, tryLock, withLock } from './lib/lock.mjs';

let scratch: string;
let lock: string;
const children: ChildProcess[] = [];
beforeEach(() => {
  scratch = mkdtempSync(join(realpathSync(tmpdir()), 'perf-harness-lock-test-'));
  lock = join(scratch, 'build.lock');
});
afterEach(() => {
  fsHooks.beforeRemove = null;
  fsHooks.beforeRename = null;
  fsHooks.afterRename = null;
  fsHooks.beforeWrite = null;
  for (const child of children.splice(0)) child.kill('SIGKILL');
  rmSync(scratch, { recursive: true, force: true });
});

/** A pid that is not running: a child that has exited. */
const deadPid = () => spawnSync(process.execPath, ['-e', '']).pid;
/** A process that is running, is not this one, and will not take or release anything. */
const otherPid = process.ppid;
/** Another such process, for a test that needs three. */
function sleeperPid(): number {
  const child = spawn('/bin/sleep', ['60'], { stdio: 'ignore' });
  children.push(child);
  return child.pid as number;
}

const takings = () => readdirSync(scratch).filter((name) => name.startsWith('build.lock.taking-'));

/**
 * Run `take` as a process that is killed in the middle of it: before it
 * writes its owner file (`write`), or with the directory ready and not yet
 * renamed to the lock (`rename`). Nothing of the taking runs after that
 * step, its own tidying up included.
 */
function killedBefore(step: 'write' | 'rename', take: () => unknown) {
  const killed = new Error('killed');
  const die = () => {
    throw killed;
  };
  if (step === 'write') fsHooks.beforeWrite = die;
  else {
    fsHooks.beforeRename = die;
    fsHooks.beforeRemove = die;
  }
  try {
    expect(take).toThrow(killed);
  } finally {
    fsHooks.beforeWrite = null;
    fsHooks.beforeRename = null;
    fsHooks.beforeRemove = null;
  }
}

describe('taking the build lock', () => {
  it('is held by one taker at a time and free again once released', () => {
    const first = tryLock(lock);
    expect(first.release).toBeTypeOf('function');
    expect(tryLock(lock)).toEqual({ heldBy: process.pid });
    first.release?.();
    expect(existsSync(lock)).toBe(false);
    const second = tryLock(lock);
    expect(second.release).toBeTypeOf('function');
    second.release?.();
    // Nothing of a taking or a release is left beside the lock.
    expect(readdirSync(scratch)).toEqual([]);
  });

  it('is taken over from a holder that is no longer running, and only from that one', () => {
    const dead = deadPid();
    expect(tryLock(lock, dead).release).toBeTypeOf('function');
    const taken = tryLock(lock);
    expect(taken.release).toBeTypeOf('function');
    // The dead holder's lock was moved aside under its own name, which is
    // what stops a late second takeover from moving the new lock.
    const buried = readdirSync(scratch).filter((name) => name.startsWith('build.lock.dead-'));
    expect(buried).toHaveLength(1);
    expect(tryLock(lock)).toEqual({ heldBy: process.pid });
    taken.release?.();

    // Kept for an hour, then cleared by the next taker.
    const old = new Date(Date.now() - 2 * 60 * 60 * 1000);
    utimesSync(join(scratch, buried[0]), old, old);
    tryLock(lock).release?.();
    expect(readdirSync(scratch)).toEqual([]);
  });

  // Three runs meet a lock whose holder died long ago. Two of them read the
  // dead owner; the first moves the lock aside, and before it does anything
  // more the third takes the free lock and clears what is past its hour.
  // The second, still acting on the dead owner, then tries its own move. It
  // must fail on the buried lock, which is what keeps that name taken.
  it('keeps a lock buried just now through a taker that passes at once, however old the lock was', () => {
    expect(tryLock(lock, deadPid()).release).toBeTypeOf('function');
    const old = new Date(Date.now() - 2 * 60 * 60 * 1000);
    utimesSync(lock, old, old);

    const firstPid = sleeperPid();
    let first: ReturnType<typeof tryLock> | undefined;
    let third: ReturnType<typeof tryLock> | undefined;
    const burying = (from: string, to: string) => from === lock && to.startsWith(`${lock}.dead-`);
    fsHooks.beforeRename = (from, to) => {
      // The second is about to move the dead lock. The first gets there first.
      if (first || !burying(from, to)) return;
      first = { heldBy: null };
      first = tryLock(lock, firstPid);
    };
    fsHooks.afterRename = (from, to) => {
      // The first has moved it, and has done nothing since.
      if (third || !burying(from, to)) return;
      third = { heldBy: null };
      third = tryLock(lock, otherPid);
    };
    const second = tryLock(lock);

    // One holder: the third. The second did not move the third's lock away
    // and take the name for itself.
    expect(third?.release).toBeTypeOf('function');
    expect(first).toEqual({ heldBy: otherPid });
    expect(second).toEqual({ heldBy: otherPid });
    expect(JSON.parse(readFileSync(join(lock, 'owner.json'), 'utf8')).pid).toBe(otherPid);
    expect(readdirSync(scratch).filter((name) => name.startsWith('build.lock.dead-'))).toHaveLength(
      1
    );
    third?.release?.();
    expect(existsSync(lock)).toBe(false);
  });

  it('removes what a process killed in the middle of taking it left behind', () => {
    const dead = deadPid();
    killedBefore('write', () => tryLock(lock, dead));
    killedBefore('rename', () => tryLock(lock, dead));
    // Two directories, one of them empty, and no lock.
    expect(takings()).toHaveLength(2);
    expect(
      takings()
        .map((name) => readdirSync(join(scratch, name)))
        .sort()
    ).toEqual([[], ['owner.json']]);
    expect(existsSync(lock)).toBe(false);

    const taken = tryLock(lock);
    expect(readdirSync(scratch)).toEqual(['build.lock']);
    taken.release?.();
    expect(readdirSync(scratch)).toEqual([]);
  });

  // A running process is stood at each step of its taking while another
  // takes the lock and clears up. Had its directory been removed, its next
  // step would fail on a path that is gone.
  it.each(['write', 'rename'] as const)(
    'never removes the taking of a running process (stood before its %s)',
    (step) => {
      // One a dead process left is there too, and the same pass clears it.
      killedBefore('write', () => tryLock(lock, deadPid()));
      const left = takings();

      let passing: ReturnType<typeof tryLock> | undefined;
      const stand = () => {
        if (passing) return;
        passing = { heldBy: null };
        const standing = takings().filter((name) => !left.includes(name));
        expect(standing).toHaveLength(1);
        passing = tryLock(lock);
        expect(takings()).toEqual(standing);
      };
      if (step === 'write') fsHooks.beforeWrite = stand;
      else fsHooks.beforeRename = stand;

      expect(tryLock(lock, otherPid)).toEqual({ heldBy: process.pid });
      expect(passing?.release).toBeTypeOf('function');
      passing?.release?.();
      expect(readdirSync(scratch)).toEqual([]);
    }
  );

  it('records who holds it and since when', () => {
    const before = Date.now();
    const taken = tryLock(lock);
    const owner = JSON.parse(readFileSync(join(lock, 'owner.json'), 'utf8'));
    expect(owner.pid).toBe(process.pid);
    expect(Date.parse(owner.since)).toBeGreaterThanOrEqual(before);
    expect(Date.parse(owner.since)).toBeLessThanOrEqual(Date.now());
    taken.release?.();
  });
});

describe('releasing the build lock', () => {
  // A removal in place takes two steps: the owner file, then the directory.
  // Between them the lock is an empty directory, which a waiter's rename
  // succeeds onto. This stands a waiter at that point of whatever removal
  // the release makes, and it must find either the holder's lock whole or
  // no lock at all.
  it('lets go in one step: a waiter that takes the lock during the release keeps it', () => {
    const held = tryLock(lock);
    let waiter: ReturnType<typeof tryLock> | undefined;
    let standing = false;
    fsHooks.beforeRemove = (path) => {
      if (standing || waiter) return;
      const inPlace = path === lock;
      if (!inPlace && !path.startsWith(`${lock}.released-`)) return;
      standing = true;
      // Where a removal of the lock itself is after its first step.
      if (inPlace) unlinkSync(join(lock, 'owner.json'));
      waiter = tryLock(lock, otherPid);
      standing = false;
    };

    expect(() => held.release?.()).not.toThrow();
    fsHooks.beforeRemove = null;

    // The waiter took it, and what the release went on to remove was not it.
    expect(waiter?.release).toBeTypeOf('function');
    expect(tryLock(lock)).toEqual({ heldBy: otherPid });
    waiter?.release?.();
    expect(readdirSync(scratch)).toEqual([]);
  });

  it('is free for the next taker as soon as it is released, with nothing left behind', () => {
    const held = tryLock(lock);
    const seen: string[][] = [];
    fsHooks.beforeRemove = () => seen.push(readdirSync(scratch).sort());
    held.release?.();
    fsHooks.beforeRemove = null;
    // What was being removed was never the lock under its own name.
    expect(seen).toHaveLength(1);
    expect(seen[0]).toHaveLength(1);
    expect(seen[0][0]).toMatch(/^build\.lock\.released-[0-9a-f]{16}$/);
    expect(readdirSync(scratch)).toEqual([]);
  });

  it('reports work that succeeded as succeeded when the lock cannot be released', async () => {
    fsHooks.beforeRename = (from, to) => {
      if (from === lock && to.startsWith(`${lock}.released-`))
        throw Object.assign(new Error('EACCES: permission denied, rename'), { code: 'EACCES' });
    };
    const said: string[] = [];
    await expect(
      withLock(lock, () => 'built', { say: (text: string) => said.push(text) })
    ).resolves.toBe('built');
    expect(said).toEqual([
      `could not release ${lock} (EACCES: permission denied, rename); it stays held until this process exits`,
    ]);

    // And a failure of the work is the failure reported, not the release's.
    rmSync(lock, { recursive: true, force: true });
    await expect(
      withLock(lock, () => {
        throw new Error('the build failed');
      })
    ).rejects.toThrow('the build failed');
  });

  it('removes what an interrupted release left, an hour later', () => {
    const held = tryLock(lock);
    // The release is cut short after its rename: the removal does nothing.
    fsHooks.beforeRemove = () => {
      throw Object.assign(new Error('interrupted'), { code: 'EINTR' });
    };
    expect(() => held.release?.()).not.toThrow();
    fsHooks.beforeRemove = null;
    const left = readdirSync(scratch);
    expect(left).toHaveLength(1);
    expect(left[0]).toMatch(/^build\.lock\.released-/);
    // The lock itself is free.
    const next = tryLock(lock);
    expect(next.release).toBeTypeOf('function');
    next.release?.();
    expect(readdirSync(scratch)).toEqual(left);

    const old = new Date(Date.now() - 2 * 60 * 60 * 1000);
    utimesSync(join(scratch, left[0]), old, old);
    tryLock(lock).release?.();
    expect(readdirSync(scratch)).toEqual([]);
  });
});

describe('waiting for the build lock', () => {
  it('runs the work holding the lock, and lets go when the work throws', async () => {
    expect(await withLock(lock, () => tryLock(lock).heldBy)).toBe(process.pid);
    expect(existsSync(lock)).toBe(false);
    // Work that is itself asynchronous holds the lock until it settles.
    expect(await withLock(lock, async () => tryLock(lock).heldBy)).toBe(process.pid);
    await expect(
      withLock(lock, () => {
        throw new Error('the build failed');
      })
    ).rejects.toThrow('the build failed');
    expect(existsSync(lock)).toBe(false);
  });

  it('waits for a holder, says whom it waits for, and runs the work once the holder is gone', async () => {
    // Held by a process that ends after a moment.
    const holder = Number(
      spawnSync('/bin/sh', ['-c', 'sleep 0.5 >/dev/null 2>&1 & echo $!'], { encoding: 'utf8' })
        .stdout
    );
    expect(tryLock(lock, holder).release).toBeTypeOf('function');
    const said: string[] = [];
    expect(
      await withLock(lock, () => 'built', { say: (text: string) => said.push(text), every: 50 })
    ).toBe('built');
    expect(said).toEqual([
      `waiting for the harness build of process ${holder}, which holds ${lock}`,
    ]);
  });

  it('leaves the event loop running while it waits', async () => {
    const held = tryLock(lock);
    let ticks = 0;
    const timer = setInterval(() => (ticks += 1), 10);
    // Released from a timer of this process: one that could not fire if the
    // wait blocked the thread.
    setTimeout(() => held.release?.(), 150);
    try {
      expect(await withLock(lock, () => 'built', { every: 20 })).toBe('built');
    } finally {
      clearInterval(timer);
    }
    expect(ticks).toBeGreaterThan(0);
  });

  it('ends on an interrupt, as the run’s signal handlers have it, without running the work', async () => {
    // Held by this process, which is running and will not let go.
    const held = tryLock(lock);
    // What `run.mjs` does: handlers first, then the build behind the lock.
    const script = `
      import { withLock } from ${JSON.stringify(pathToFileURL(join(import.meta.dirname, 'lib', 'lock.mjs')).href)};
      for (const name of ['SIGINT', 'SIGTERM', 'SIGHUP'])
        process.on(name, () => {
          console.log('interrupted (' + name + ')');
          process.exit(130);
        });
      await withLock(${JSON.stringify(lock)}, () => console.log('built'), {
        say: (text) => console.log(text),
        every: 50,
      });
    `;
    const child = spawn(process.execPath, ['--input-type=module', '-e', script], {
      stdio: ['ignore', 'pipe', 'inherit'],
    });
    children.push(child);
    const lines: string[] = [];
    const waiting = new Promise<void>((resolve) => {
      createInterface({ input: child.stdout! }).on('line', (line) => {
        lines.push(line);
        if (line.startsWith('waiting')) resolve();
      });
    });
    const exited = new Promise<{ code: number | null; signal: string | null }>((resolve) =>
      child.on('exit', (code, signal) => resolve({ code, signal }))
    );
    await waiting;
    child.kill('SIGINT');

    expect(await exited).toEqual({ code: 130, signal: null });
    expect(lines).toEqual([
      `waiting for the harness build of process ${process.pid}, which holds ${lock}`,
      'interrupted (SIGINT)',
    ]);
    // It took nothing and left nothing: the lock is the holder's alone.
    expect(readdirSync(scratch)).toEqual(['build.lock']);
    held.release?.();
  }, 60_000);

  it('gives up after its bound on a holder that never lets go, and names it', async () => {
    // What a dead holder's lock looks like once its process id has been
    // given to something else: held by a process that is running, and that
    // will never release it.
    const held = tryLock(lock, otherPid);
    const since = JSON.parse(readFileSync(join(lock, 'owner.json'), 'utf8')).since;
    const command = spawnSync('ps', ['-o', 'command=', '-p', String(otherPid)], {
      encoding: 'utf8',
    }).stdout.trim();
    expect(command).not.toBe('');
    const work = vi.fn(() => 'built');
    const said: string[] = [];
    const started = Date.now();

    const failure = await withLock(lock, work, {
      say: (text: string) => said.push(text),
      every: 20,
      timeout: 200,
    }).then(
      () => null,
      (error: Error & { code?: string }) => error
    );

    expect(failure?.code).toBe(LOCK_WAIT_EXPIRED);
    expect(failure?.message).toBe(
      `gave up after 0s waiting for ${lock}, held by process ${otherPid} (\`${command}\`), which took it at ${since}. ` +
        `If that process is not a harness build (the process id of a build that was killed can since have gone to another process), remove ${lock} and run again.`
    );
    expect(Date.now() - started).toBeGreaterThanOrEqual(200);
    expect(work).not.toHaveBeenCalled();
    expect(said).toEqual([
      `waiting for the harness build of process ${otherPid}, which holds ${lock}`,
    ]);
    // Given up on, not taken over: the holder is still the holder.
    expect(tryLock(lock)).toEqual({ heldBy: otherPid });
    held.release?.();
  });

  it('fails at once with a bound of zero', async () => {
    const held = tryLock(lock, otherPid);
    const started = Date.now();
    await expect(withLock(lock, () => 'built', { timeout: 0 })).rejects.toMatchObject({
      code: LOCK_WAIT_EXPIRED,
    });
    expect(Date.now() - started).toBeLessThan(1000);
    // A free lock is taken whatever the bound.
    held.release?.();
    expect(await withLock(lock, () => 'built', { timeout: 0 })).toBe('built');
  });
});
