import { spawn, spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import { createServer, type IncomingMessage } from 'node:http';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  createClient,
  TOKEN_COOKIE,
  TOKEN_HEADER,
  tokenCookie,
  tokenCookieScript,
} from './driver/app.mjs';
import { connection } from './driver/cdp.mjs';
import { HOOKS_WANTED, olderHooksWarning } from './driver/hooks.mjs';
import { agentLoad, backendResult, frontendResult, mountedRows } from './driver/measure.mjs';
import { expectedChunksPerSecond, overloadReason } from './driver/overload.mjs';
import { cpuMeter } from './driver/sampler.mjs';
import { plan } from './driver/scenarios.mjs';
import { startStaleness } from './driver/staleness.mjs';
import { outdatedFor, stampsIn, storedStamps } from './driver/stamps.mjs';
import { distribution, histogramMs, processUsage } from './driver/stats.mjs';
import { compareChat, subtitleRound, verifyChat } from './driver/verify.mjs';
import {
  builtMarker,
  cachedBinaryDir,
  chooseTargetDir,
  defaultTargetDir,
  replaceCachedBinary,
} from './lib/build.mjs';
import { tryLock, withLock } from './lib/lock.mjs';
import { parseParams } from './lib/params.mjs';
import { compare, differingParams, summarize } from './lib/report.mjs';
import { isolationProblem, scrubbedEnv } from './lib/safety.mjs';

let scratch: string;
beforeEach(() => {
  scratch = mkdtempSync(join(realpathSync(tmpdir()), 'perf-harness-test-'));
});
afterEach(() => {
  rmSync(scratch, { recursive: true, force: true });
});

describe('parameters', () => {
  it('has a default for every knob and reads the ones given', () => {
    const params = parseParams([
      '--projects',
      '5',
      '--keep',
      '--local-storage',
      'a=b=c',
      '--local-storage',
      'x=',
    ]);
    expect(params.projects).toBe(5);
    expect(params.sessionsPerProject).toBe(4);
    expect(params.keep).toBe(true);
    expect(params.scenarios).toEqual(['subtitles', 'chat', 'both', 'hidden']);
    expect(params.localStorage).toEqual({ a: 'b=c', x: '' });
    expect(params).toMatchObject({
      viewportHeight: 1000,
      toolStepMs: 500,
      promptBytes: 0,
      startTimeout: 240,
      minRateShare: 0.5,
      skipVerify: false,
      targetDir: '',
    });
  });

  it('takes any localStorage key, and none by default', () => {
    // No key is known by name: the switch a run was first written for is gone.
    expect(parseParams([]).localStorage).toEqual({});
    expect(parseParams(['--local-storage', 'some:new-switch=on']).localStorage).toEqual({
      'some:new-switch': 'on',
    });
  });

  it('reads the knobs added for tall projects, tool cadence and prompt size', () => {
    expect(
      parseParams([
        '--viewport-height',
        '6000',
        '--tool-step-ms',
        '100',
        '--prompt-bytes',
        '12000',
        '--min-rate-share',
        '0',
        '--skip-verify',
      ])
    ).toMatchObject({
      viewportHeight: 6000,
      toolStepMs: 100,
      promptBytes: 12000,
      minRateShare: 0,
      skipVerify: true,
    });
    expect(() => parseParams(['--tool-step-ms', '0'])).toThrow(/at least 1/);
    expect(() => parseParams(['--min-rate-share', '2'])).toThrow(/share/);
    expect(() => parseParams(['--frontend', 'desktop', '--viewport-height', '4000'])).toThrow(
      /browser window/
    );
  });

  it('refuses what it does not know and a load on one project', () => {
    expect(() => parseParams(['--nope', '1'])).toThrow(/unknown argument/);
    expect(() => parseParams(['--scenarios', 'subtitles,other'])).toThrow(/unknown scenario/);
    expect(() => parseParams(['--projects', '1'])).toThrow(/at least 2/);
    expect(() => parseParams(['--local-storage', 'novalue'])).toThrow(/key=value/);
  });
});

describe('the isolation check', () => {
  /** A stand-in for the user's home with a `.staged` in it, and a run directory beside it. */
  function layout() {
    const userHome = join(scratch, 'user');
    mkdirSync(join(userHome, '.staged', 'packages'), { recursive: true });
    const runDir = join(scratch, 'run');
    mkdirSync(join(runDir, 'home'), { recursive: true });
    return { userHome, runDir, home: join(runDir, 'home') };
  }

  it('lets a run start when its data directory is inside its own directory', () => {
    const { userHome, runDir, home } = layout();
    expect(isolationProblem({ runDir, home, userHome, executables: ['/usr/bin/true'] })).toBeNull();
  });

  it('refuses the real home, a run directory inside the real data, and a HOME outside the run', () => {
    const { userHome, runDir, home } = layout();
    expect(isolationProblem({ runDir, home: userHome, userHome })).toMatch(
      /not inside the run directory/
    );
    const inside = join(userHome, '.staged', 'run');
    mkdirSync(join(inside, 'home'), { recursive: true });
    expect(isolationProblem({ runDir: inside, home: join(inside, 'home'), userHome })).toMatch(
      /inside the real data directory/
    );
    expect(
      isolationProblem({ runDir: scratch, home: join(scratch, 'run', 'home'), userHome })
    ).toMatch(/contains the real/);
    expect(isolationProblem({ runDir, home: join(scratch, 'elsewhere'), userHome })).toMatch(
      /not inside the run directory/
    );
    expect(isolationProblem({ runDir, home, userHome })).toBeNull();
  });

  it('follows a symlink that leads the data directory back to the real one', () => {
    const { userHome, runDir, home } = layout();
    symlinkSync(join(userHome, '.staged'), join(home, '.staged'));
    expect(isolationProblem({ runDir, home, userHome })).toMatch(/not inside the run directory/);
  });

  it('refuses a program that lives in the real data directory', () => {
    const { userHome, runDir, home } = layout();
    const node = join(userHome, '.staged', 'packages', 'node');
    writeFileSync(node, '');
    expect(isolationProblem({ runDir, home, userHome, executables: [node] })).toMatch(
      /Node outside it/
    );
  });

  it('gives what it starts nothing of the caller’s environment', () => {
    const env = scrubbedEnv({ runDir: '/run', home: '/run/home' });
    expect(env.HOME).toBe('/run/home');
    expect(env.PATH).toBe('/run/bin:/usr/bin:/bin:/usr/sbin:/sbin');
    for (const key of Object.keys(env))
      expect(String(env[key])).not.toContain(process.env.HOME ?? '\0');
  });
});

describe('the cargo target directory', () => {
  it('is the harness’s own unless one is named, and never the tree’s', () => {
    const tree = join(scratch, 'shared-target');
    mkdirSync(tree);
    expect(chooseTargetDir({ requested: '', treeTargetDir: tree })).toBe(defaultTargetDir());
    const other = join(scratch, 'other');
    expect(chooseTargetDir({ requested: other, treeTargetDir: tree })).toBe(other);
    expect(() => chooseTargetDir({ requested: tree, treeTargetDir: tree })).toThrow(
      /ordinary builds of this tree/
    );
    // The same directory by another name is still refused.
    symlinkSync(tree, join(scratch, 'link'));
    expect(() =>
      chooseTargetDir({ requested: join(scratch, 'link'), treeTargetDir: tree })
    ).toThrow(/ordinary builds of this tree/);
  });
});

describe('the build cache', () => {
  it('drops every port’s marker of a profile when that profile’s binary is replaced', () => {
    const cache = join(scratch, 'cache');
    const from = join(scratch, 'cargo-target', 'debug');
    mkdirSync(join(from, 'resources'), { recursive: true });
    writeFileSync(join(from, 'Staged'), 'built for 47612');
    writeFileSync(join(from, 'resources', 'guide.md'), 'guide');
    // What an earlier build for another port left: its binary and its marker.
    mkdirSync(cachedBinaryDir(cache, 'debug'), { recursive: true });
    writeFileSync(join(cachedBinaryDir(cache, 'debug'), 'Staged'), 'built for 47611');
    writeFileSync(join(cachedBinaryDir(cache, 'debug'), 'stale'), 'from the older build');
    for (const [profile, port] of [
      ['debug', 47611],
      ['debug', 47612],
      ['release', 47611],
    ] as const)
      writeFileSync(builtMarker(cache, profile, port), 'then');

    const binaryDir = replaceCachedBinary({ from, cache, profile: 'debug' });

    expect(binaryDir).toBe(cachedBinaryDir(cache, 'debug'));
    expect(readFileSync(join(binaryDir, 'Staged'), 'utf8')).toBe('built for 47612');
    expect(readFileSync(join(binaryDir, 'resources', 'guide.md'), 'utf8')).toBe('guide');
    expect(existsSync(join(binaryDir, 'stale'))).toBe(false);
    // Neither port may be skipped to until its build has finished and written
    // its own marker; the other profile's binary was not touched.
    expect(existsSync(builtMarker(cache, 'debug', 47611))).toBe(false);
    expect(existsSync(builtMarker(cache, 'debug', 47612))).toBe(false);
    expect(existsSync(builtMarker(cache, 'release', 47611))).toBe(true);
  });
});

describe('the build lock', () => {
  /** A pid that is not running: a child that has exited. */
  const deadPid = () => spawnSync(process.execPath, ['-e', '']).pid;

  it('is held by one taker at a time and free again once released', () => {
    const lock = join(scratch, 'build.lock');
    const first = tryLock(lock);
    expect(first.release).toBeTypeOf('function');
    expect(tryLock(lock)).toEqual({ heldBy: process.pid });
    first.release?.();
    expect(existsSync(lock)).toBe(false);
    const second = tryLock(lock);
    expect(second.release).toBeTypeOf('function');
    second.release?.();
    // Nothing of a taking is left beside the lock.
    expect(readdirSync(scratch)).toEqual([]);
  });

  it('is taken over from a holder that is no longer running, and only from that one', () => {
    const lock = join(scratch, 'build.lock');
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

  it('runs the work holding the lock, waits for a holder, and lets go when the work throws', () => {
    const lock = join(scratch, 'build.lock');
    expect(withLock(lock, () => tryLock(lock).heldBy)).toBe(process.pid);
    expect(() =>
      withLock(lock, () => {
        throw new Error('the build failed');
      })
    ).toThrow('the build failed');
    expect(existsSync(lock)).toBe(false);

    // Held by a process that ends after a moment: the wait is announced, and
    // the work runs once the holder is gone. Not a child of this process: the
    // wait does not return to the event loop, so a child that exited would
    // never be reaped and would still count as running.
    const holder = Number(
      spawnSync('/bin/sh', ['-c', 'sleep 0.5 >/dev/null 2>&1 & echo $!'], { encoding: 'utf8' })
        .stdout
    );
    expect(tryLock(lock, holder).release).toBeTypeOf('function');
    const said: string[] = [];
    expect(
      withLock(lock, () => 'built', { say: (text: string) => said.push(text), every: 50 })
    ).toBe('built');
    expect(said).toEqual([
      `waiting for the harness build of process ${holder}, which holds ${lock}`,
    ]);
  });
});

describe('the run’s token', () => {
  let seen: { url: string; headers: IncomingMessage['headers'] }[];
  let server: ReturnType<typeof createServer>;
  let port: number;
  beforeEach(async () => {
    seen = [];
    // The app's rule: nothing without the token.
    server = createServer((request, response) => {
      seen.push({ url: request.url ?? '', headers: request.headers });
      if (request.headers[TOKEN_HEADER] !== 'the-run-token') {
        response.writeHead(401).end('perf-harness: this port takes the run’s token');
        return;
      }
      response.writeHead(200, { 'content-type': 'application/json' }).end('{"ok":true}');
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    port = (server.address() as AddressInfo).port;
  });
  afterEach(() => new Promise((resolve) => server.close(resolve)));

  it('goes with every request the driver makes, commands and harness routes alike', async () => {
    const api = createClient({ port, token: 'the-run-token' });
    expect(await api.invoke('list_projects')).toEqual({ ok: true });
    expect(await api.metrics()).toEqual({ ok: true });
    await api.resetMetrics();
    expect(await api.outdated({ a: 3 }, 1700000000123)).toEqual({ ok: true });
    await api.evalInWebviews('1');
    await api.takeResults();
    expect(seen.map((request) => request.url)).toEqual([
      '/api/invoke/list_projects',
      '/perf/metrics',
      '/perf/metrics/reset',
      '/perf/outdated?at=1700000000123',
      '/perf/eval',
      '/perf/results',
    ]);
    for (const request of seen) expect(request.headers[TOKEN_HEADER]).toBe('the-run-token');
    // The driver's commands are still told from the page's.
    expect(seen[0].headers['x-perf-driver']).toBe('1');
  });

  it('fails a command the app refuses, with the status', async () => {
    const api = createClient({ port, token: 'another-run' });
    await expect(api.invoke('list_projects')).rejects.toThrow(/list_projects: 401/);
    await expect(api.metrics()).rejects.toThrow(/\/perf\/metrics: 401/);
  });

  it('reaches the page as a cookie no script can read, set before the page loads', () => {
    const api = createClient({ port, token: 'the-run-token' });
    expect(tokenCookie(api)).toEqual({
      name: TOKEN_COOKIE,
      value: 'the-run-token',
      url: `http://127.0.0.1:${port}`,
      httpOnly: true,
      sameSite: 'Strict',
    });
    // The desktop webview has no such door: the document sets it, for the
    // session only.
    expect(tokenCookieScript(api)).toBe(
      `document.cookie = "${TOKEN_COOKIE}=the-run-token; path=/; SameSite=Strict";`
    );
  });
});

describe('older hooks', () => {
  it('says what a result lacks for each version behind, and nothing when current', () => {
    expect(olderHooksWarning(HOOKS_WANTED)).toBeNull();
    expect(olderHooksWarning(HOOKS_WANTED + 1)).toBeNull();
    const two = olderHooksWarning(2);
    expect(two).toContain(`version 2 and this harness expects ${HOOKS_WANTED}`);
    expect(two).toContain('answered any local process');
    expect(two).toContain('no piecesBehind');
    expect(two).not.toContain('no subtitle staleness figure');
    const one = olderHooksWarning(1);
    expect(one).toContain('no subtitle staleness figure');
    expect(one).toContain('answered any local process');
  });
});

describe('the DevTools connection', () => {
  /** A socket that records what is sent and lets a test deliver events. */
  function fakeSocket() {
    const handlers = new Map<string, ((event: unknown) => void)[]>();
    const sent: { id: number; method: string }[] = [];
    return {
      sent,
      addEventListener: (name: string, handler: (event: unknown) => void) =>
        handlers.set(name, [...(handlers.get(name) ?? []), handler]),
      send: (text: string) => sent.push(JSON.parse(text)),
      emit: (name: string, event: unknown = {}) =>
        (handlers.get(name) ?? []).forEach((handler) => handler(event)),
    };
  }

  it('answers a call and hands events to their listeners', async () => {
    const socket = fakeSocket();
    const { send, on } = connection(socket);
    const events: unknown[] = [];
    on('Page.loaded', (params: unknown) => events.push(params));
    const call = send('Runtime.evaluate', { expression: '1' });
    socket.emit('message', { data: JSON.stringify({ id: socket.sent[0].id, result: { v: 1 } }) });
    socket.emit('message', { data: JSON.stringify({ method: 'Page.loaded', params: { n: 2 } }) });
    expect(await call).toEqual({ v: 1 });
    expect(events).toEqual([{ n: 2 }]);
  });

  it('rejects the calls in flight when the socket closes, and every call after', async () => {
    const socket = fakeSocket();
    const { send } = connection(socket);
    const inFlight = send('Runtime.evaluate');
    socket.emit('close', { code: 1006 });
    await expect(inFlight).rejects.toThrow(/Runtime\.evaluate: the DevTools connection closed/);
    await expect(send('Page.navigate')).rejects.toThrow(/connection closed/);
    expect(socket.sent).toHaveLength(1);
  });

  it('rejects a call the browser never answers', async () => {
    vi.useFakeTimers();
    try {
      const { send } = connection(fakeSocket(), { timeout: 5000 });
      const call = send('Runtime.evaluate');
      const outcome = expect(call).rejects.toThrow(/no answer from the browser in 5s/);
      await vi.advanceTimersByTimeAsync(5000);
      await outcome;
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('stamps', () => {
  const rows = [
    { role: 'user', content: 'Load test pf-tag=s0p0b1 pfm-xs0p0b1-99-1700000000999' },
    { role: 'assistant', content: 'pfm-xs0p0b1-1-1700000000100 a\npfm-xs0p0b1-2-1700000000200 b' },
    { role: 'tool_call', content: '{"title":"Run pfm-ts0p0b1-3-1700000000300"}' },
    { role: 'tool_result', content: 'out\npfm-os0p0b1-4-1700000000400' },
    { role: 'assistant', content: 'pfm-xs0p0b10-7-1700000000700 another session' },
  ];

  it('reads every stamp of a text, the three kinds apart', () => {
    expect(stampsIn('a pfm-xs1-4-1700000000100 b pfm-os1-5-1700000000250')).toEqual([
      { kind: 'x', tag: 's1', seq: 4, sent: 1700000000100 },
      { kind: 'o', tag: 's1', seq: 5, sent: 1700000000250 },
    ]);
    expect(stampsIn('cut off pfm-xs1-4-17000')).toEqual([]);
  });

  it('takes from a transcript what a row can show: assistant text and tool calls of that session', () => {
    const stored = storedStamps(rows, 's0p0b1');
    expect([...stored.text]).toEqual([
      [1, 1700000000100],
      [2, 1700000000200],
    ]);
    expect([...stored.tool]).toEqual([[3, 1700000000300]]);
    // Not the tool output, not the user's row, not a session whose tag only begins the same.
    expect(stored.newest).toEqual({ kind: 't', seq: 3, sent: 1700000000300 });
    expect(storedStamps(rows, 's0p0b9').newest).toBeNull();
  });

  it('measures how out of date a row is from when the first newer piece was stored', () => {
    // The row shows seq 25, a tool call. Its output steps took 26 to 30 and
    // the text after it 31 and 32: seven sequence numbers on, two pieces a
    // row can show, which is what the app counts.
    const answer = {
      newestSeq: 32,
      newerSeq: 31,
      newerStoredMs: 1700000000400,
      newerCount: 2,
      floor: false,
    };
    // The store has held seq 31 for 350 ms.
    expect(outdatedFor(answer, 1700000000750)).toEqual({
      behind: true,
      ms: 350,
      pieces: 2,
      floor: false,
    });
    // Stored after the page was read: the row was not behind when it was looked at.
    const current = { behind: false, ms: 0, pieces: 0, floor: false };
    expect(outdatedFor(answer, 1700000000390)).toEqual(current);
    // Nothing newer stored.
    expect(
      outdatedFor(
        { newestSeq: 8, newerSeq: null, newerStoredMs: null, newerCount: 0, floor: false },
        1
      )
    ).toEqual(current);
    // Further behind than the app keeps: least values.
    expect(
      outdatedFor(
        {
          newestSeq: 400,
          newerSeq: 145,
          newerStoredMs: 1700000000000,
          newerCount: 256,
          floor: true,
        },
        1700000002000
      )
    ).toEqual({ behind: true, ms: 2000, pieces: 256, floor: true });
    // Hooks that do not count: behind, by an unknown number of pieces, and
    // never by a difference of sequence numbers.
    expect(
      outdatedFor(
        { newestSeq: 32, newerSeq: 31, newerStoredMs: 1700000000400, floor: false },
        1700000000750
      )
    ).toEqual({ behind: true, ms: 350, pieces: null, floor: false });
  });
});

describe('subtitle staleness', () => {
  const sessions = [
    { tag: 'a', branchId: 'ba' },
    { tag: 'b', branchId: 'bb' },
    { tag: 'c', branchId: 'bc' },
    { tag: 'd', branchId: 'bd' },
  ];
  const row = (tag: string, seq: number | null, mounted = true) => ({
    tag,
    card: true,
    row: mounted,
    kind: seq === null ? null : 'x',
    seq,
    sent: seq === null ? null : 1000 + seq,
  });

  it('asks the app about what each mounted row shows, page first', async () => {
    const order: string[] = [];
    const asked: Record<string, number>[] = [];
    const askedAt: (number | undefined)[] = [];
    const frontend = {
      evaluate: async () => {
        order.push('page');
        return {
          at: 5000,
          rows: [row('a', 7), row('b', 5), row('c', null, false), row('d', null)],
        };
      },
    };
    const api = {
      outdated: async (shown: Record<string, number>, at?: number) => {
        order.push('app');
        asked.push(shown);
        askedAt.push(at);
        return {
          // Seq 8 was stored 600 ms before the page was read. The row shows
          // 7 and the newest is 14, but 9 to 13 are a tool call's output:
          // two pieces a row can show, not seven.
          a: { newestSeq: 14, newerSeq: 8, newerStoredMs: 4400, newerCount: 2, floor: false },
          // Up to date.
          b: { newestSeq: 5, newerSeq: null, newerStoredMs: null, newerCount: 0, floor: false },
          // Mounted and showing nothing, with three pieces stored.
          d: { newestSeq: 3, newerSeq: 1, newerStoredMs: 4900, newerCount: 3, floor: false },
        };
      },
    };
    const sampler = startStaleness({ api, frontend, sessions, every: 5 });
    await vi.waitFor(() => expect(order.length).toBeGreaterThan(3));
    const figures = await sampler.stop();
    expect(order.slice(0, 2)).toEqual(['page', 'app']);
    // The unmounted row is not asked about; a row with no stamp is asked as 0.
    expect(asked[0]).toEqual({ a: 7, b: 5, d: 0 });
    // With when the page was read, so the app counts what was stored by then.
    expect(askedAt[0]).toBe(5000);
    expect(figures).toMatchObject({ rows: 3, floors: 0, over1s: 0, stoppedEarly: null });
    expect(figures?.outdatedMs).toMatchObject({ max: 600, p50: 100 });
    // The app's count of showable pieces, not `newestSeq - seq`.
    expect(figures?.piecesBehind).toMatchObject({ max: 3, p50: 2 });
    expect(figures?.piecesBehind?.count).toBe((figures?.samples ?? 0) * 3);
    expect(figures?.outdatedMs?.count).toBe((figures?.samples ?? 0) * 3);
    expect(figures?.upToDate).toBe(figures?.samples);
    expect(figures?.nothingShown).toBe(figures?.samples);
  });

  it('is absent on hooks that cannot answer, and says when the page was lost', async () => {
    const frontend = { evaluate: async () => ({ at: 1, rows: [row('a', 1)] }) };
    const old = startStaleness({
      api: { outdated: async () => null },
      frontend,
      sessions,
      every: 5,
    });
    expect(await old.stop()).toBeNull();

    let reads = 0;
    const lost = startStaleness({
      api: {
        outdated: async () => ({
          a: { newestSeq: 1, newerSeq: null, newerStoredMs: null, floor: false },
        }),
      },
      frontend: {
        evaluate: async () => {
          if (++reads > 2) throw new Error('the DevTools connection closed');
          return { at: 1, rows: [row('a', 1)] };
        },
      },
      sessions,
      every: 5,
    });
    await vi.waitFor(() => expect(reads).toBeGreaterThan(2));
    expect(await lost.stop()).toMatchObject({
      samples: 2,
      stoppedEarly: 'the DevTools connection closed',
    });
  });

  it('has no pieces figure from hooks that do not count them, and still times the rows', async () => {
    let asks = 0;
    const sampler = startStaleness({
      api: {
        outdated: async () => {
          asks += 1;
          return { a: { newestSeq: 9, newerSeq: 4, newerStoredMs: 700, floor: false } };
        },
      },
      frontend: { evaluate: async () => ({ at: 1000, rows: [row('a', 3)] }) },
      sessions,
      every: 5,
    });
    await vi.waitFor(() => expect(asks).toBeGreaterThan(1));
    const figures = await sampler.stop();
    expect(figures?.piecesBehind).toBeNull();
    expect(figures?.outdatedMs).toMatchObject({ max: 300 });
  });
});

describe('shown against stored', () => {
  const session = { id: 'sid', tag: 'a', branchId: 'ba' };
  const transcript = (seqs: number[]) => [
    {
      role: 'assistant',
      content: seqs.map((seq) => `pfm-xa-${seq}-${1700000000000 + seq}`).join('\n'),
    },
  ];
  const api = (rows: unknown[]) => ({ invoke: async () => rows });

  it('passes a chat that shows the transcript from its oldest row on', () => {
    const stored = new Map([1, 2, 3, 4, 5].map((seq) => [seq, 1700000000000 + seq]));
    const shown = new Map([3, 4, 5].map((seq) => [seq, 1700000000000 + seq]));
    expect(compareChat({ shown, stored })).toMatchObject({
      shown: 3,
      stored: 5,
      newestShown: 5,
      newestStored: 5,
      unknown: [],
      missing: [],
    });
  });

  it('names a stamp the store does not hold, one it holds with another time, and a hole', () => {
    const stored = new Map([1, 2, 3, 4, 5].map((seq) => [seq, 1700000000000 + seq]));
    const shown = new Map([
      [2, 1700000000002],
      [3, 1700000000999],
      [5, 1700000000005],
      [6, 1700000000006],
    ]);
    const compared = compareChat({ shown, stored });
    expect(compared.unknown).toEqual([3, 6]);
    expect(compared.missing).toEqual([4]);
  });

  it('fails a chat that never comes to show the end of the transcript', async () => {
    const frontend = {
      evaluate: async () => [
        [1, 1700000000001],
        [2, 1700000000002],
      ],
    };
    const check = await verifyChat({
      api: api(transcript([1, 2, 3])),
      frontend,
      session,
      timeout: 20,
    });
    expect(check).toMatchObject({ ok: false, missing: [3], newestShown: 2, newestStored: 3 });
    const good = await verifyChat({ api: api(transcript([1, 2])), frontend, session, timeout: 20 });
    expect(good.ok).toBe(true);
    // An empty chat is not a pass.
    const empty = await verifyChat({
      api: api(transcript([1])),
      frontend: { evaluate: async () => [] },
      session,
      timeout: 20,
    });
    expect(empty.ok).toBe(false);
  });

  it('passes a subtitle that comes to show what the store held, and times it', async () => {
    let reads = 0;
    const frontend = {
      // Mounted; then a piece behind; then the newest.
      evaluate: async () => {
        reads += 1;
        const seq = reads < 3 ? 2 : 3;
        return {
          at: 0,
          rows: [{ tag: 'a', card: true, row: true, kind: 'x', seq, sent: 1700000000000 + seq }],
        };
      },
    };
    const round = await subtitleRound({
      api: api(transcript([1, 2, 3])),
      frontend,
      sessions: [session],
      timeout: 2000,
    });
    expect(round).toMatchObject({ rows: 1, late: [], unknown: [] });
    expect(round.caughtUpMs).toHaveLength(1);
  });

  it('fails a subtitle that stays behind, and one that shows a stamp the store does not hold', async () => {
    const stuck = await subtitleRound({
      api: api(transcript([1, 2, 3])),
      frontend: {
        evaluate: async () => ({
          at: 0,
          rows: [{ tag: 'a', card: true, row: true, kind: 'x', seq: 1, sent: 1700000000001 }],
        }),
      },
      sessions: [session],
      timeout: 150,
    });
    expect(stuck.late).toEqual(['a: shows seq 1, the store held x3 when the round began']);
    expect(stuck.unknown).toEqual([]);

    const wrong = await subtitleRound({
      api: api(transcript([1, 2, 3])),
      frontend: {
        evaluate: async () => ({
          at: 0,
          rows: [{ tag: 'a', card: true, row: true, kind: 'x', seq: 3, sent: 1700000000777 }],
        }),
      },
      sessions: [session],
      timeout: 150,
    });
    expect(wrong.late).toEqual([]);
    expect(wrong.unknown).toEqual(['pfm-xa-3-1700000000777']);
  });
});

describe('the overload rule', () => {
  it('knows what a session writes when nothing holds it back', () => {
    expect(expectedChunksPerSecond({ chunksPerSecond: 4, toolEvery: 0, toolMs: 2000 })).toBe(4);
    // 24 chunks at 4 a second, then a 2 s tool call: 25 chunks in 8 s.
    expect(expectedChunksPerSecond({ chunksPerSecond: 4, toolEvery: 25, toolMs: 2000 })).toBe(
      3.125
    );
  });

  it('stops a window whose agents or sampler fall behind, and only then', () => {
    const window = { expectedPerSecond: 100, seconds: 10, minShare: 0.5 };
    expect(overloadReason({ ...window, chunks: 900, samples: 10 })).toBeNull();
    expect(overloadReason({ ...window, chunks: 300, samples: 10 })).toMatch(
      /30 chunks a second, 30% of the 100 asked for/
    );
    expect(overloadReason({ ...window, chunks: 900, samples: 3 })).toMatch(/3 samples in 10s/);
    expect(overloadReason({ ...window, chunks: 0, samples: 0, minShare: 0 })).toBeNull();
  });
});

describe('figures', () => {
  it('reads percentiles from a histogram as bucket upper bounds, capped at the maximum', () => {
    const histogram = {
      count: 10,
      sumUs: 30_000,
      maxUs: 9_000,
      buckets: [
        [1_000, 8],
        [16_000, 2],
      ],
    };
    expect(histogramMs(histogram)).toMatchObject({ count: 10, p50: 1, p95: 9, max: 9, mean: 3 });
    expect(histogramMs({ count: 0, sumUs: 0, maxUs: 0, buckets: [] })).toBeNull();
    expect(distribution([])).toBeNull();
    expect(distribution([1, 2, 3, 4])).toMatchObject({ count: 4, p50: 3, max: 4, mean: 2.5 });
  });

  it('takes CPU between the first and last sample, as a share of the time between them', () => {
    const samples = [
      { t: 10_400, cpuSeconds: 10, rssKb: 102_400, processes: 1 },
      { t: 11_400, cpuSeconds: 10.5, rssKb: 153_600, processes: 2 },
      { t: 12_900, cpuSeconds: 11, rssKb: 204_800, processes: 2 },
    ];
    // One CPU second over the 2.5 s the samples span: 40 %, whatever the
    // length of the window they were picked for.
    expect(processUsage(samples)).toEqual({
      cpuSeconds: 1,
      cpuPercent: 40,
      spanSeconds: 2.5,
      rssMaxMb: 200,
      rssMeanMb: 150,
      processesMax: 2,
      samples: 3,
    });
    expect(processUsage(samples.slice(2))).toBeNull();
    const together = samples.slice(0, 2).map((sample) => ({ ...sample, t: 5 }));
    expect(processUsage(together)?.cpuPercent).toBeNull();
  });

  it('keeps the CPU of a process that exits, and counts a new one from nothing', () => {
    const meter = cpuMeter();
    const p = (pid: number, cpuSeconds: number) => ({ pid, cpuSeconds });
    // Two agents that have run for a while.
    expect(meter([p(10, 5), p(11, 3)])).toBe(8);
    expect(meter([p(10, 6), p(11, 3.5)])).toBe(9.5);
    // One exits. A sum over the living would fall to 6.5, and the window's
    // difference would be negative; its 3.5 s stay counted.
    expect(meter([p(10, 6.5)])).toBe(10);
    // A git process seen once, with all it has used so far.
    expect(meter([p(10, 6.5), p(12, 0.25)])).toBe(10.25);
    expect(meter([p(10, 7)])).toBe(10.75);
    // A pid used again by another process starts over; so does one `ps`
    // lists with no time left as it exits.
    expect(meter([p(10, 7), p(12, 0.1)])).toBe(10.85);
    expect(meter([p(10, 0)])).toBe(10.85);
    expect(meter([])).toBe(10.85);
    // Two meters do not share what they have seen.
    expect(cpuMeter()([p(10, 7)])).toBe(7);
  });

  it('counts the stream frames a client took apart from the ones none did', () => {
    const histogram = { count: 0, sumUs: 0, maxUs: 0, buckets: [] };
    const metrics = (hooks: number, frames: object) => ({
      hooks,
      commands: {},
      streamFrames: frames,
      storeLock: { wait: histogram, hold: histogram },
      stampToStored: { text: histogram, tool: histogram },
    });
    const usage = { app: [], children: [], browser: [] };
    const result = (hooks: number, frames: object) =>
      backendResult({
        metrics: metrics(hooks, frames),
        usage,
        frontendKind: 'browser',
        streamSupported: true,
      }).streamTotals;
    expect(
      result(3, {
        'full.delta': { frames: 40, bytes: 4000, dropped: 2 },
        'summary.delta': { frames: 60, bytes: 600, dropped: 0 },
      })
    ).toEqual({ frames: 100, bytes: 4600, dropped: 2 });
    // Hooks that counted a frame before it was sent cannot say.
    expect(result(2, { 'full.delta': { frames: 40, bytes: 4000 } })).toEqual({
      frames: 40,
      bytes: 4000,
      dropped: null,
    });
  });

  it('counts a row as mounted only when it was on the page at both ends of the window', () => {
    const row = (tag: string, mounted: boolean) => ({ tag, card: true, row: mounted });
    expect(
      mountedRows(
        [row('a', true), row('b', true), row('c', false)],
        [row('a', true), row('b', false), row('c', true)]
      )
    ).toEqual({ mounted: 1, mountedAtStart: 2, mountedAtEnd: 2, notMounted: ['b', 'c'] });
  });

  it('sets what the agents wrote against the rate asked for', () => {
    const load = agentLoad(
      { a: { chunks: 10, toolCalls: 0, plans: 0, bytes: 0 } },
      { a: { chunks: 40, toolCalls: 1, plans: 0, bytes: 900 } },
      10,
      4
    );
    expect(load).toMatchObject({ chunks: 30, chunksPerSecond: 3, rateShare: 0.75 });
  });

  it('totals what the page received both ways, decoded and on the wire', () => {
    const probe = {
      gaps: [],
      longTasks: [],
      timerLag: [],
      latency: { subtitle: { x: [], t: [] }, chat: { x: [], t: [] } },
      frameLatency: { subtitle: { x: [], t: [] }, chat: { x: [], t: [] } },
      mutations: 0,
      longTaskSupported: true,
      hidden: false,
      tauri: false,
    };
    const network = (socketCompressed: boolean) => ({
      commands: { poll_session: { count: 2, bytes: 1000, wireBytes: 300 } },
      events: {
        'session-stream': { frames: 3, bytes: 600 },
        'session-status-changed': { frames: 1, bytes: 50 },
      },
      stream: { 'summary.delta': { frames: 3, bytes: 600 } },
      socketCompressed,
    });
    const usage = { browser: [] };
    const plain = frontendResult({
      collected: { probe, network: network(false), renderer: null },
      usage,
      seconds: 10,
      streamSupported: true,
    });
    expect(plain.commandTotals).toEqual({ count: 2, bytes: 1000, wireBytes: 300 });
    expect(plain.streamTotals).toEqual({ frames: 3, bytes: 600, wireBytes: 600 });
    expect(plain.received).toEqual({ bytes: 1650, wireBytes: 950, socketCompressed: false });
    // A compressed socket's wire size is not known, and is not guessed.
    const compressed = frontendResult({
      collected: { probe, network: network(true), renderer: null },
      usage,
      seconds: 10,
      streamSupported: true,
    });
    expect(compressed.streamTotals?.wireBytes).toBeNull();
    expect(compressed.received?.wireBytes).toBeNull();
  });
});

describe('the scenario plan', () => {
  const projects = [0, 1, 2].map((p) => ({ branches: [0, 1].map((b) => ({ id: `p${p}b${b}` })) }));

  it('spreads the running sessions across the projects', () => {
    const sessions = plan({ name: 'subtitles', index: 0, projects, running: 4 });
    expect(sessions.map((s) => s.tag)).toEqual(['s0p0b0', 's0p1b0', 's0p2b0', 's0p0b1']);
    expect(sessions.every((s) => s.profile === 'background')).toBe(true);
  });

  it('streams fast in the session whose chat is open, alone in its project for `chat`', () => {
    const sessions = plan({ name: 'chat', index: 1, projects, running: 0 });
    expect(sessions.filter((s) => s.project === 0).map((s) => s.tag)).toEqual(['s1p0b0']);
    expect(sessions.find((s) => s.tag === 's1p0b0')?.profile).toBe('chat');
    expect(sessions).toHaveLength(5);
  });
});

describe('reports', () => {
  const result = (
    label: string,
    stream: { frames: number; bytes: number } | null,
    cpuSeconds: number
  ) => ({
    label,
    tree: { head: 'abcdef0123456789', branch: 'main', dirty: false },
    frontend: 'browser',
    streamSupported: stream !== null,
    params: { ...parseParams([]), label },
    host: { loadAverageStart: [1, 1, 1], loadAverageEnd: [1, 1, 1] },
    complete: true,
    failure: null as null | { phase: string; message: string },
    notRun: [] as string[],
    warnings: [] as string[],
    checks: { ok: true, failed: [] as string[] },
    scenarios: {
      subtitles: {
        load: { sessions: 4 },
        backend: { process: { cpuSeconds }, streamTotals: stream },
      },
    } as Record<string, object>,
    safety: {
      runDir: '/run',
      dataDir: '/run/home/.staged',
      realStagedDir: '/home/.staged',
      openFileSamples: 3,
      openUnderRealStaged: [],
      unexpectedExecutables: [],
      leftoverProcesses: [],
    },
  });

  it('leaves the stream figures out of a summary of a tree without the stream', () => {
    const summary = summarize(result('old', null, 1));
    expect(summary).toContain('not in this tree');
    expect(summary).not.toContain('stream frames sent');
    expect(summarize(result('new', { frames: 7, bytes: 70 }, 1))).toMatch(
      /stream frames sent\s+\|\s+7 \|/
    );
  });

  it('calls two runs like for like when they differ only in where they were built', () => {
    const before = result('before', { frames: 7, bytes: 70 }, 2);
    const after = result('after', { frames: 7, bytes: 70 }, 3);
    // The README's recipe: the second run names another tree.
    after.params = {
      ...after.params,
      appDir: '/other/checkout/apps/staged',
      targetDir: '/tmp/other-target',
      port: 47612,
      skipBuild: true,
    };
    expect(differingParams(before.params, after.params)).toEqual([]);
    expect(compare(before, after)).not.toContain('not like for like');

    // A different load is still called out, among the keys that are not.
    after.params = { ...after.params, rate: 8, sessionsPerProject: 6 };
    expect(differingParams(before.params, after.params)).toEqual(['sessionsPerProject', 'rate']);
    expect(compare(before, after)).toContain(
      'warning: the two runs differ in sessionsPerProject, rate; their figures are not like for like'
    );
  });

  it('shows the difference, and `absent` on the side that has no figure', () => {
    const text = compare(result('old', null, 2), result('new', { frames: 7, bytes: 70 }, 3));
    expect(text).toMatch(/app CPU s\s+\|\s+2 \|\s+3 \|\s+\+1 \|\s+\+50% \|/);
    expect(text).toMatch(/stream frames sent\s+\|\s+absent \|\s+7 \|/);
  });

  it('keeps what a failed run measured, and says what failed and what was not run', () => {
    const partial = result('partial', { frames: 7, bytes: 70 }, 1);
    const failure = {
      phase: 'scenario chat: starting 5 sessions',
      message:
        '1 of 5 sessions were not streaming after 240s (the app has 4 agent processes): s1p1b0 (session abc): status running, 0 messages',
    };
    partial.complete = false;
    partial.failure = failure;
    partial.scenarios.chat = { about: 'an open chat', failure };
    partial.notRun = ['both', 'hidden'];
    const summary = summarize(partial);
    expect(summary).toContain('**FAILED while scenario chat: starting 5 sessions: 1 of 5 sessions');
    expect(summary).toContain('from the scenario measured before that (subtitles)');
    expect(summary).toContain('Not run: both, hidden.');
    expect(summary).toContain('- chat, while scenario chat: starting 5 sessions: 1 of 5');
    // The scenario that was measured still has its column, and the failed one has none.
    expect(summary).toMatch(/\|\s+\| subtitles \|\n/);
    expect(summary).toMatch(/stream frames sent\s+\|\s+7 \|/);
    expect(compare(partial, result('whole', { frames: 7, bytes: 70 }, 1))).toContain(
      'warning: A failed: 1 of 5 sessions'
    );
  });

  it('reports the shown-against-stored checks and what to read before the figures', () => {
    const checked = result('checked', { frames: 7, bytes: 70 }, 1);
    checked.scenarios.subtitles = {
      ...checked.scenarios.subtitles,
      checks: {
        subtitles: { ok: true, rounds: 3, rows: 12, caughtUpMsMax: 410, late: [], unknown: [] },
        chat: {
          ok: false,
          shown: 40,
          stored: 42,
          newestShown: 40,
          newestStored: 42,
          unknown: [],
          missing: [41, 42],
        },
      },
    };
    checked.checks = { ok: false, failed: ['subtitles: chat'] };
    checked.warnings = ['subtitles: 3 of the viewed project’s 25 rows were not mounted'];
    const summary = summarize(checked);
    expect(summary).toContain(
      '- subtitles, subtitles: ok; 12 rows over 3 rounds, each showing what the store held within 410 ms'
    );
    expect(summary).toContain(
      '- subtitles, chat: FAILED; 40 text stamps shown, 42 stored, newest 40 against 42; 2 stored and not shown (first seq 41)'
    );
    expect(summary).toContain('## Read before trusting the figures');
    expect(summary).toContain('3 of the viewed project’s 25 rows were not mounted');
  });
});

describe('the fake agent', () => {
  it('streams stamped text and tool calls for a tagged prompt, and stops when told', async () => {
    const config = join(scratch, 'config.json');
    const profile = {
      chunksPerSecond: 200,
      chunkChars: 20,
      paragraphEvery: 4,
      toolEvery: 5,
      toolMs: 40,
      toolStepMs: 10,
      toolBytes: 100,
      largeToolEvery: 0,
      largeToolBytes: 0,
      planEvery: 7,
      turnSeconds: 60,
    };
    writeFileSync(config, JSON.stringify({ profiles: { fast: profile } }));
    const agent = spawn(
      process.execPath,
      [
        join(import.meta.dirname, 'agent', 'fake-agent.cjs'),
        '--config',
        config,
        '--state',
        scratch,
      ],
      { stdio: ['pipe', 'pipe', 'inherit'] }
    );
    const exited = new Promise((resolve) => agent.once('exit', resolve));
    const lines = createInterface({ input: agent.stdout })[Symbol.asyncIterator]();
    const send = (message: object) =>
      agent.stdin.write(JSON.stringify({ jsonrpc: '2.0', ...message }) + '\n');
    const next = async () => JSON.parse((await lines.next()).value);

    send({ id: 1, method: 'initialize', params: {} });
    expect((await next()).result.agentInfo.name).toBe('staged-perf-fake-agent');
    send({ id: 2, method: 'session/new', params: { cwd: scratch } });
    const { sessionId } = (await next()).result;
    send({
      id: 3,
      method: 'session/prompt',
      params: { sessionId, prompt: [{ type: 'text', text: 'go pf-tag=t1 pf-profile=fast' }] },
    });

    const kinds = new Set<string>();
    const stamps: string[] = [];
    /** Per tool call: the distinct stamps of the call itself, and how many times its output grew. */
    const calls = new Map<string, { stamps: Set<string>; growths: number }>();
    let stopReason = null;
    while (stopReason === null) {
      const message = await next();
      if (message.id === 3) stopReason = message.result.stopReason;
      else {
        const update = message.params.update;
        kinds.add(update.sessionUpdate);
        const found = JSON.stringify(message).match(/pfm-[xto]t1-\d+-\d{13}/g) ?? [];
        stamps.push(...new Set<string>(found));
        if (update.sessionUpdate === 'tool_call') {
          calls.set(update.toolCallId, { stamps: new Set(found), growths: 0 });
          // The call is stamped `t`, and nothing else is.
          expect(found.every((stamp: string) => stamp.startsWith('pfm-tt1-'))).toBe(true);
        } else if (update.sessionUpdate === 'tool_call_update') {
          expect(found).toHaveLength(1);
          expect(found[0]).toMatch(/^pfm-ot1-/);
          if (update.status === 'in_progress') calls.get(update.toolCallId)!.growths += 1;
        } else if (found.length) {
          expect(found[0]).toMatch(/^pfm-xt1-/);
        }
        if (kinds.size === 5 && stamps.length > 10)
          send({ method: 'session/cancel', params: { sessionId } });
      }
    }
    expect(stopReason).toBe('cancelled');
    expect([...kinds].sort()).toEqual([
      'agent_message_chunk',
      'plan',
      'session_info_update',
      'tool_call',
      'tool_call_update',
    ]);
    const sequence = stamps.map((stamp) => Number(stamp.split('-')[2]));
    expect(sequence).toEqual([...sequence].sort((a, b) => a - b));
    // One stamp per tool call, whichever field a view reads its label from,
    // and its output grows every `toolStepMs`: 40 ms in steps of 10.
    const first = [...calls.values()][0];
    expect(first.stamps.size).toBe(1);
    expect(first.growths).toBe(4);
    expect(JSON.parse(readFileSync(join(scratch, 't1.json'), 'utf8'))).toMatchObject({
      tag: 't1',
      stop: 'cancelled',
    });

    agent.stdin.end();
    expect(await exited).toBe(0);
  }, 20_000);
});
