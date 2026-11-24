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
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { plan } from './driver/scenarios.mjs';
import { distribution, histogramMs, processUsage } from './driver/stats.mjs';
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
    });
    expect(processUsage(samples.slice(1), 2)).toBeNull();
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
    scenarios: {
      subtitles: {
        load: { sessions: 4 },
        backend: { process: { cpuSeconds }, streamTotals: stream },
      },
    },
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
});

describe('the fake agent', () => {
  it('streams stamped text and tool calls for a tagged prompt, and stops when told', async () => {
    const config = join(scratch, 'config.json');
    const profile = {
      chunksPerSecond: 200,
      chunkChars: 20,
      paragraphEvery: 4,
      toolEvery: 5,
      toolMs: 0,
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
    let stopReason = null;
    while (stopReason === null) {
      const message = await next();
      if (message.id === 3) stopReason = message.result.stopReason;
      else {
        kinds.add(message.params.update.sessionUpdate);
        stamps.push(...(JSON.stringify(message).match(/pfm-[xt]t1-\d+-\d{13}/g) ?? []));
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
    expect(JSON.parse(readFileSync(join(scratch, 't1.json'), 'utf8'))).toMatchObject({
      tag: 't1',
      stop: 'cancelled',
    });

    agent.stdin.end();
    expect(await exited).toBe(0);
  }, 20_000);
});
