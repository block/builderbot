import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DoctorCheck, DoctorReport } from '../../api/commands';
import type { Session } from '../../types';
import type { AgentLoginOutcome } from '../doctor/agentLogin.svelte';

type LoginSession = Pick<Session, 'provider' | 'status' | 'errorMessage'>;

function session(overrides: Partial<LoginSession> = {}): LoginSession {
  return {
    provider: 'claude',
    status: 'error',
    errorMessage: 'Authentication required',
    ...overrides,
  };
}

function report(overrides: Partial<DoctorCheck> = {}): DoctorReport {
  return {
    checks: [
      {
        id: 'ai-agent-claude',
        label: 'Claude Code',
        status: 'warn',
        message: 'Not authenticated',
        fixUrl: null,
        fixCommand: null,
        fixType: null,
        path: '/usr/local/bin/claude-agent-acp',
        bridgePath: null,
        rawOutput: null,
        authStatus: 'notAuthenticated',
        loginCommand: 'claude-agent-acp --cli auth login',
        installedVersion: null,
        latestVersion: null,
        updateAvailable: null,
        installSource: null,
        selfUpdating: null,
        main: null,
        bridge: null,
        ...overrides,
      },
    ],
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

let input: { active: boolean; sessionId: string; session: LoginSession | null };
let doctorState: { report: DoctorReport | null; loading: boolean };
let agentLogin: { checkId: string | null; running: boolean };
let runChecks: ReturnType<typeof vi.fn<() => Promise<void>>>;
let attachAgentLogin: ReturnType<typeof vi.fn<(id: string) => Promise<AgentLoginOutcome | null>>>;
let startAgentLogin: ReturnType<typeof vi.fn<(id: string) => Promise<AgentLoginOutcome>>>;
let effects: Array<() => void>;

beforeEach(() => {
  vi.resetModules();
  input = { active: true, sessionId: 'session-1', session: session() };
  doctorState = { report: null, loading: false };
  agentLogin = { checkId: null, running: false };
  runChecks = vi.fn().mockResolvedValue(undefined);
  attachAgentLogin = vi.fn().mockResolvedValue(null);
  startAgentLogin = vi.fn().mockResolvedValue('cancelled');
  effects = [];
  // Vitest has no Svelte plugin: capture effects and explicitly rerun them
  // after input changes. Getters must read live data without rune transforms.
  vi.stubGlobal('$effect', (effect: () => void) => effects.push(effect));
  vi.doMock('../doctor/doctor.svelte', () => ({ doctorState, runChecks }));
  vi.doMock('../doctor/agentLogin.svelte', () => ({
    agentLogin,
    attachAgentLogin,
    startAgentLogin,
  }));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.doUnmock('../doctor/doctor.svelte');
  vi.doUnmock('../doctor/agentLogin.svelte');
});

async function create() {
  const { createSessionLoginController } = await import('./sessionLogin.svelte');
  return createSessionLoginController({
    getActive: () => input.active,
    getSessionId: () => input.sessionId,
    getSession: () => input.session,
  });
}

function runEffects() {
  effects.forEach((effect) => effect());
}

describe('session login controller', () => {
  it('reads provider, report and shared running changes through live getters', async () => {
    const controller = await create();
    expect(controller.checkId).toBe('ai-agent-claude');
    expect(controller.canLogin).toBe(false);
    expect(controller.running).toBe(false);

    doctorState.report = report();
    expect(controller.canLogin).toBe(true);
    doctorState.report = report({ id: 'ai-agent-codex' });
    expect(controller.canLogin).toBe(false);
    input.session = session({ provider: 'codex' });
    expect(controller.checkId).toBe('ai-agent-codex');
    expect(controller.canLogin).toBe(true);

    agentLogin.running = true;
    agentLogin.checkId = 'ai-agent-claude';
    expect(controller.running).toBe(false);
    agentLogin.checkId = 'ai-agent-codex';
    expect(controller.running).toBe(true);
    agentLogin.running = false;
    expect(controller.running).toBe(false);
    doctorState.report = null;
    expect(controller.canLogin).toBe(false);

    input.session = session({ provider: null });
    expect(controller.checkId).toBeNull();
    input.session = null;
    expect(controller.checkId).toBeNull();
    expect(controller.canLogin).toBe(false);
    expect(controller.running).toBe(false);
    expect(runChecks).not.toHaveBeenCalled();
    expect(attachAgentLogin).not.toHaveBeenCalled();
  });

  it.each([
    { name: 'inactive pane', active: false },
    { name: 'missing session ID', sessionId: '' },
    { name: 'missing session', session: null },
    { name: 'queued session', session: session({ status: 'queued' }) },
    { name: 'running session', session: session({ status: 'running' }) },
    { name: 'completed session', session: session({ status: 'completed' }) },
    { name: 'unrelated error', session: session({ errorMessage: 'Connection refused' }) },
    { name: 'missing error', session: session({ errorMessage: null }) },
    {
      name: 'unrelated cancellation',
      session: session({ status: 'cancelled', errorMessage: 'User stopped the session' }),
    },
  ])(
    'gates both effects for $name without consuming the attempt',
    async ({ name: _, ...patch }) => {
      Object.assign(input, patch);
      await create();
      runEffects();
      expect(runChecks).not.toHaveBeenCalled();
      expect(attachAgentLogin).not.toHaveBeenCalled();

      input = { active: true, sessionId: 'session-1', session: session() };
      runEffects();
      expect(runChecks).toHaveBeenCalledTimes(1);
      expect(attachAgentLogin).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
    }
  );

  it.each(['error', 'cancelled'] as const)(
    'recovers an authentication failure in %s',
    async (status) => {
      input.session = session({ status });
      await create();
      runEffects();
      runEffects();
      expect(runChecks).toHaveBeenCalledTimes(1);
      expect(attachAgentLogin).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
    }
  );

  it('can request a report without a provider, but cannot attach', async () => {
    input.session = session({ provider: null });
    await create();
    runEffects();
    expect(runChecks).toHaveBeenCalledTimes(1);
    expect(attachAgentLogin).not.toHaveBeenCalled();
  });

  it.each(['loading', 'existing report'] as const)(
    'defers the initial report for %s without deferring attach',
    async (gate) => {
      doctorState.loading = gate === 'loading';
      doctorState.report = gate === 'existing report' ? report() : null;
      await create();
      runEffects();
      expect(runChecks).not.toHaveBeenCalled();
      expect(attachAgentLogin).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');

      doctorState.loading = false;
      doctorState.report = null;
      runEffects();
      expect(runChecks).toHaveBeenCalledTimes(1);
      expect(attachAgentLogin).toHaveBeenCalledTimes(1);
    }
  );

  it('does not spin or retry a failed scan on reopen, but scans a new session', async () => {
    const scan = deferred<void>();
    runChecks.mockImplementationOnce(async () => {
      doctorState.loading = true;
      await scan.promise;
      // runChecks swallows scan failures, leaving no report.
      doctorState.loading = false;
    });
    await create();
    runEffects();
    runEffects();
    expect(runChecks).toHaveBeenCalledTimes(1);
    scan.resolve(undefined);
    await runChecks.mock.results[0].value;
    runEffects();
    runEffects();
    input.active = false;
    runEffects();
    input.active = true;
    runEffects();
    expect(runChecks).toHaveBeenCalledTimes(1);
    expect(attachAgentLogin).toHaveBeenCalledTimes(2);

    input.sessionId = 'session-2';
    runEffects();
    expect(runChecks).toHaveBeenCalledTimes(2);
  });

  it.each([
    { name: 'no matching check', overrides: { id: 'ai-agent-codex' } },
    { name: 'unknown authentication', overrides: { authStatus: 'unknown' as const } },
    { name: 'no login command', overrides: { loginCommand: null } },
  ])('attaches independently of eligibility: $name', async ({ overrides }) => {
    doctorState.report = report(overrides);
    const controller = await create();
    expect(controller.canLogin).toBe(false);
    runEffects();
    expect(attachAgentLogin).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
    expect(runChecks).not.toHaveBeenCalled();
  });

  it('probes once per open and again on reopening or switching sessions', async () => {
    doctorState.report = report();
    await create();
    runEffects();
    await Promise.resolve(); // Let the null attach result settle before another flush.
    runEffects();
    expect(attachAgentLogin).toHaveBeenCalledTimes(1);
    input.active = false;
    runEffects();
    expect(attachAgentLogin).toHaveBeenCalledTimes(1);
    input.active = true;
    runEffects();
    input.sessionId = 'session-2';
    input.session = session({ provider: 'codex' });
    runEffects();
    runEffects();
    expect(attachAgentLogin.mock.calls).toEqual([
      ['ai-agent-claude'],
      ['ai-agent-claude'],
      ['ai-agent-codex'],
    ]);
  });

  it('defers attach while any shared login runs without consuming the attempt', async () => {
    doctorState.report = report();
    agentLogin = { checkId: 'ai-agent-codex', running: true };
    await create();
    runEffects();
    expect(attachAgentLogin).not.toHaveBeenCalled();
    agentLogin.running = false;
    runEffects();
    runEffects();
    expect(attachAgentLogin).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
  });

  it('starts only with a provider and an eligible report', async () => {
    const controller = await create();
    await controller.start();
    doctorState.report = report();
    input.session = null;
    await controller.start();
    input.session = session({ provider: null });
    await controller.start();
    input.session = session();
    doctorState.report = report({ loginCommand: null });
    await controller.start();
    expect(startAgentLogin).not.toHaveBeenCalled();
    doctorState.report = report();
    await controller.start();
    expect(startAgentLogin).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
  });

  it.each(['claude', 'codex'])(
    'blocks starts globally while %s runs, but presents running per provider',
    async (provider) => {
      doctorState.report = report();
      agentLogin = { checkId: `ai-agent-${provider}`, running: true };
      const controller = await create();
      expect(controller.canLogin).toBe(true);
      expect(controller.running).toBe(provider === 'claude');
      await controller.start();
      expect(startAgentLogin).not.toHaveBeenCalled();
      agentLogin.running = false;
      await controller.start();
      expect(startAgentLogin).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
    }
  );

  it('does not refresh when attach finds no login', async () => {
    doctorState.report = report();
    await create();
    runEffects();
    await Promise.resolve();
    expect(attachAgentLogin).toHaveBeenCalledTimes(1);
    expect(runChecks).not.toHaveBeenCalled();
  });

  describe.each(['start', 'attach'] as const)('%s completion', (entry) => {
    it.each(['completed', 'cancelled'] as const)(
      'refreshes only for completed: %s',
      async (outcome) => {
        doctorState.report = report();
        const controller = await create();
        const action = entry === 'start' ? startAgentLogin : attachAgentLogin;
        action.mockResolvedValueOnce(outcome);
        await (entry === 'start' ? controller.start() : runEffects());
        expect(action).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
        expect(runChecks).toHaveBeenCalledTimes(outcome === 'completed' ? 1 : 0);
      }
    );

    it('swallows rejection without refreshing', async () => {
      doctorState.report = report();
      const controller = await create();
      const action = entry === 'start' ? startAgentLogin : attachAgentLogin;
      action.mockRejectedValueOnce(new Error('Login failed'));
      await (entry === 'start' ? controller.start() : runEffects());
      expect(action).toHaveBeenCalledTimes(1);
      expect(runChecks).not.toHaveBeenCalled();
    });

    it.each(['close', 'switch session'] as const)(
      'refreshes the global report even after %s while completion was pending',
      async (change) => {
        doctorState.report = report();
        const controller = await create();
        const pending = deferred<AgentLoginOutcome>();
        const action = entry === 'start' ? startAgentLogin : attachAgentLogin;
        action.mockReturnValueOnce(pending.promise);
        const settled = entry === 'start' ? controller.start() : runEffects();
        agentLogin.running = true;
        agentLogin.checkId = 'ai-agent-claude';

        if (change === 'close') input.active = false;
        else {
          input.sessionId = 'session-2';
          input.session = session({ provider: 'codex' });
        }
        runEffects();
        expect(agentLogin.running).toBe(true);
        expect(runChecks).not.toHaveBeenCalled();
        pending.resolve('completed');
        await pending.promise;
        await settled;
        expect(runChecks).toHaveBeenCalledTimes(1);
        expect(action).toHaveBeenCalledExactlyOnceWith('ai-agent-claude');
      }
    );
  });
});
