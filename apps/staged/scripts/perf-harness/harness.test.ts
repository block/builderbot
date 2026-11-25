import { spawn } from 'node:child_process';
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { connection } from './driver/cdp.mjs';
import { agentLoad, frontendResult, mountedRows } from './driver/measure.mjs';
import { expectedChunksPerSecond, overloadReason } from './driver/overload.mjs';
import { plan } from './driver/scenarios.mjs';
import { startStaleness } from './driver/staleness.mjs';
import { outdatedFor, stampsIn, storedStamps } from './driver/stamps.mjs';
import { distribution, histogramMs, processUsage } from './driver/stats.mjs';
import { compareChat, subtitleRound, verifyChat } from './driver/verify.mjs';
import { chooseTargetDir, defaultTargetDir } from './lib/build.mjs';
import { parseParams } from './lib/params.mjs';
import { compare, summarize } from './lib/report.mjs';
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
    const answer = { newestSeq: 8, newerSeq: 6, newerStoredMs: 1700000000400, floor: false };
    // The store has held seq 6 for 350 ms and the row still shows seq 5, three pieces back.
    expect(outdatedFor(5, answer, 1700000000750)).toEqual({ ms: 350, pieces: 3, floor: false });
    // Stored after the page was read: the row was not behind when it was looked at.
    expect(outdatedFor(5, answer, 1700000000390)).toEqual({ ms: 0, pieces: 0, floor: false });
    // Nothing newer stored.
    expect(
      outdatedFor(8, { newestSeq: 8, newerSeq: null, newerStoredMs: null, floor: false }, 1)
    ).toEqual({ ms: 0, pieces: 0, floor: false });
    // A row that shows nothing is behind by everything stored.
    expect(
      outdatedFor(
        null,
        { newestSeq: 4, newerSeq: 1, newerStoredMs: 1700000000000, floor: true },
        1700000002000
      )
    ).toEqual({ ms: 2000, pieces: 4, floor: true });
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
      outdated: async (shown: Record<string, number>) => {
        order.push('app');
        asked.push(shown);
        return {
          // Seq 8 was stored 600 ms before the page was read; the row shows 7 of 10.
          a: { newestSeq: 10, newerSeq: 8, newerStoredMs: 4400, floor: false },
          // Up to date.
          b: { newestSeq: 5, newerSeq: null, newerStoredMs: null, floor: false },
          // Mounted and showing nothing, with three pieces stored.
          d: { newestSeq: 3, newerSeq: 1, newerStoredMs: 4900, floor: false },
        };
      },
    };
    const sampler = startStaleness({ api, frontend, sessions, every: 5 });
    await vi.waitFor(() => expect(order.length).toBeGreaterThan(3));
    const figures = await sampler.stop();
    expect(order.slice(0, 2)).toEqual(['page', 'app']);
    // The unmounted row is not asked about; a row with no stamp is asked as 0.
    expect(asked[0]).toEqual({ a: 7, b: 5, d: 0 });
    expect(figures).toMatchObject({ rows: 3, floors: 0, over1s: 0, stoppedEarly: null });
    expect(figures?.outdatedMs).toMatchObject({ max: 600, p50: 100 });
    expect(figures?.piecesBehind?.max).toBe(3);
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

  it('takes CPU as the difference across the window', () => {
    const samples = [
      { t: 0, cpuSeconds: 10, rssKb: 102_400, processes: 1 },
      { t: 1, cpuSeconds: 11, rssKb: 204_800, processes: 2 },
    ];
    expect(processUsage(samples, 2)).toEqual({
      cpuSeconds: 1,
      cpuPercent: 50,
      rssMaxMb: 200,
      rssMeanMb: 150,
      processesMax: 2,
      samples: 2,
    });
    expect(processUsage(samples.slice(1), 2)).toBeNull();
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
