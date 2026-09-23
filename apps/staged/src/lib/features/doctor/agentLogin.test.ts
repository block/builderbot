import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DoctorLoginOutput } from '../../api/commands';

/** A registration made by the module under test through `listenToEvent`. */
interface Registration {
  event: string;
  callback: (payload: DoctorLoginOutput) => void;
  onEstablished?: () => void;
  unlisten: ReturnType<typeof vi.fn>;
}

describe('agentLogin', () => {
  let startDoctorLogin: ReturnType<typeof vi.fn>;
  let sendDoctorLoginCode: ReturnType<typeof vi.fn>;
  let registrations: Registration[];

  beforeEach(() => {
    vi.resetModules();
    // The store is a .svelte.ts module compiled without the Svelte plugin
    // here, so the rune calls resolve to this pass-through global.
    vi.stubGlobal('$state', (initial: unknown) => initial);

    registrations = [];
    startDoctorLogin = vi.fn().mockResolvedValue(undefined);
    sendDoctorLoginCode = vi.fn().mockResolvedValue(undefined);
    vi.doMock('../../api/commands', () => ({ startDoctorLogin, sendDoctorLoginCode }));
    vi.doMock('../../transport', () => ({
      listenToEvent: (
        event: string,
        callback: (payload: DoctorLoginOutput) => void,
        opts?: { onEstablished?: () => void }
      ) => {
        const unlisten = vi.fn();
        registrations.push({ event, callback, onEstablished: opts?.onEstablished, unlisten });
        return unlisten;
      },
    }));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.doUnmock('../../api/commands');
    vi.doUnmock('../../transport');
  });

  function load() {
    return import('./agentLogin.svelte');
  }

  /** The single live registration, failing loudly if there isn't exactly one. */
  function only(): Registration {
    const live = registrations.filter((r) => r.unlisten.mock.calls.length === 0);
    expect(live).toHaveLength(1);
    return live[0];
  }

  function line(checkId: string, text: string): DoctorLoginOutput {
    return { checkId, line: text, done: false, error: null };
  }

  function done(checkId: string, error: string | null = null): DoctorLoginOutput {
    return { checkId, line: null, done: true, error };
  }

  it('pulls the sign-in URL out of the line the CLI actually prints', async () => {
    const { extractLoginUrl } = await load();

    expect(
      extractLoginUrl(
        'If the browser didn’t open, visit: ' +
          'https://claude.ai/oauth/authorize?code=true&client_id=abc&response_type=code'
      )
    ).toBe('https://claude.ai/oauth/authorize?code=true&client_id=abc&response_type=code');
    // Sentence punctuation is not part of the address.
    expect(extractLoginUrl('Open https://example.com/login.')).toBe('https://example.com/login');
    expect(extractLoginUrl('Opening browser to sign in…')).toBeNull();
  });

  it('starts the fix only once the listener is live', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    // Registration is asynchronous: a `done` emitted before it went live would
    // be lost, so nothing may be started until `onEstablished`.
    expect(startDoctorLogin).not.toHaveBeenCalled();
    expect(agentLogin.running).toBe(true);

    const registration = only();
    expect(registration.event).toBe('doctor-login-output');
    registration.onEstablished?.();
    expect(startDoctorLogin).toHaveBeenCalledWith('ai-agent-claude');

    // A web-socket reconnect re-establishes the same listener; the fix is
    // already running, and starting a second would be refused by the backend.
    registration.onEstablished?.();
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);

    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBeUndefined();
    expect(agentLogin.running).toBe(false);
    expect(registration.unlisten).toHaveBeenCalled();
  });

  it('records the sign-in URL and the output tail, ignoring other checks', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    registration.callback(line('ai-agent-claude', 'Opening browser to sign in…'));
    registration.callback(
      line('ai-agent-claude', 'If the browser didn’t open, visit: https://claude.ai/oauth')
    );
    registration.callback(line('ai-agent-codex', 'https://auth.openai.com/other'));
    registration.callback(done('ai-agent-codex'));

    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.output).toEqual([
      'Opening browser to sign in…',
      'If the browser didn’t open, visit: https://claude.ai/oauth',
    ]);
    // Another check's `done` must not finish this login.
    expect(agentLogin.running).toBe(true);

    registration.callback(done('ai-agent-claude'));
    await settled;
  });

  it('reports a failed login on the record and to the caller', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    registration.callback(done('ai-agent-claude', 'doctor: fix timed out after 600s'));

    await expect(settled).rejects.toThrow('doctor: fix timed out after 600s');
    expect(agentLogin.error).toBe('doctor: fix timed out after 600s');
    expect(agentLogin.running).toBe(false);
  });

  it('reports a login that never spawned instead of waiting on it', async () => {
    startDoctorLogin.mockRejectedValue(new Error('No login fix available for ai-agent-goose'));
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-goose');
    only().onEstablished?.();

    await expect(settled).rejects.toThrow('No login fix available for ai-agent-goose');
    expect(agentLogin.running).toBe(false);
  });

  it('keeps the code box open after a send, so a rejected code can be retried', async () => {
    const { agentLogin, startAgentLogin, submitAgentLoginCode } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();

    agentLogin.code = '  abc-123  ';
    await submitAgentLoginCode();
    expect(sendDoctorLoginCode).toHaveBeenCalledWith('ai-agent-claude', 'abc-123');
    // Only the sent text is cleared: the CLI re-prompts on a code it rejects,
    // and nothing in the stream announces that prompt.
    expect(agentLogin.code).toBe('');
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.error).toBeNull();

    // Nothing to send is not an error, and neither is a finished login.
    await submitAgentLoginCode();
    registration.callback(done('ai-agent-claude'));
    await settled;
    agentLogin.code = 'late';
    await submitAgentLoginCode();
    expect(sendDoctorLoginCode).toHaveBeenCalledTimes(1);
  });

  it('surfaces a refused code without ending the login', async () => {
    sendDoctorLoginCode.mockRejectedValue(new Error('No active login for ai-agent-claude'));
    const { agentLogin, startAgentLogin, submitAgentLoginCode } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    agentLogin.code = 'abc-123';
    await submitAgentLoginCode();

    expect(agentLogin.error).toBe('No active login for ai-agent-claude');
    expect(agentLogin.sending).toBe(false);
    expect(agentLogin.running).toBe(true);

    registration.callback(done('ai-agent-claude'));
    await settled;
  });

  it('refuses a second login rather than taking the record from the first', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();

    await expect(startAgentLogin('ai-agent-codex')).rejects.toThrow(/already running/);
    expect(agentLogin.checkId).toBe('ai-agent-claude');
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);

    registration.callback(done('ai-agent-claude'));
    await settled;
  });

  it('scopes the record to one check and clears only a finished one', async () => {
    const { agentLogin, agentLoginFor, clearAgentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();

    expect(agentLoginFor('ai-agent-claude')).toBe(agentLogin);
    expect(agentLoginFor('ai-agent-codex')).toBeNull();
    expect(agentLoginFor(null)).toBeNull();

    // A running login owns the record; clearing it would hide live output.
    clearAgentLogin('ai-agent-claude');
    expect(agentLoginFor('ai-agent-claude')).toBe(agentLogin);

    registration.callback(done('ai-agent-claude', 'login failed'));
    await expect(settled).rejects.toThrow('login failed');

    clearAgentLogin('ai-agent-codex');
    expect(agentLogin.error).toBe('login failed');
    clearAgentLogin('ai-agent-claude');
    expect(agentLoginFor('ai-agent-claude')).toBeNull();
    expect(agentLogin.error).toBeNull();
  });
});
