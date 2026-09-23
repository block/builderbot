import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DoctorLoginOutput, DoctorLoginStatus } from '../../api/commands';

/** A registration made by the module under test through `listenToEvent`. */
interface Registration {
  event: string;
  callback: (payload: DoctorLoginOutput) => void;
  onEstablished?: () => void;
  unlisten: ReturnType<typeof vi.fn>;
}

const AUTHORIZE_LINE = 'If the browser didn’t open, visit: https://claude.ai/oauth';

describe('agentLogin', () => {
  let startDoctorLogin: ReturnType<typeof vi.fn>;
  let sendDoctorLoginCode: ReturnType<typeof vi.fn>;
  let cancelDoctorLogin: ReturnType<typeof vi.fn>;
  let doctorLoginStatus: ReturnType<typeof vi.fn>;
  let registrations: Registration[];
  /** Sequence number the next `line()` carries, as the backend would number it. */
  let nextSeq: number;

  beforeEach(() => {
    vi.resetModules();
    // The store is a .svelte.ts module compiled without the Svelte plugin
    // here, so the rune calls resolve to this pass-through global.
    vi.stubGlobal('$state', (initial: unknown) => initial);

    registrations = [];
    nextSeq = 0;
    startDoctorLogin = vi.fn().mockResolvedValue('started');
    sendDoctorLoginCode = vi.fn().mockResolvedValue(undefined);
    cancelDoctorLogin = vi.fn().mockResolvedValue(true);
    doctorLoginStatus = vi.fn().mockResolvedValue(idle());
    vi.doMock('../../api/commands', () => ({
      startDoctorLogin,
      sendDoctorLoginCode,
      cancelDoctorLogin,
      doctorLoginStatus,
    }));
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

  function line(checkId: string, text: string, seq: number = nextSeq++): DoctorLoginOutput {
    return { checkId, line: text, seq, done: false, error: null, cancelled: false };
  }

  function done(
    checkId: string,
    error: string | null = null,
    cancelled = false
  ): DoctorLoginOutput {
    return { checkId, line: null, seq: nextSeq, done: true, error, cancelled };
  }

  function idle(): DoctorLoginStatus {
    return { running: false, output: [], nextSeq: 0 };
  }

  function running(output: string[]): DoctorLoginStatus {
    return { running: true, output, nextSeq: output.length };
  }

  /** Let the promise chains behind a start or a status answer run to the end. */
  function flush(): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, 0));
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
    await flush();

    // A web-socket reconnect re-establishes the same listener; the fix is
    // already running, and starting a second would be refused by the backend.
    // The reconnect re-syncs from the backend instead, which still has the run.
    doctorLoginStatus.mockResolvedValue(running([]));
    registration.onEstablished?.();
    await flush();
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);
    expect(agentLogin.running).toBe(true);

    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
    expect(registration.unlisten).toHaveBeenCalled();
  });

  it('records the sign-in URL and the output tail, ignoring other checks', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    registration.callback(line('ai-agent-claude', 'Opening browser to sign in…'));
    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE));
    registration.callback(line('ai-agent-codex', 'https://auth.openai.com/other'));
    registration.callback(done('ai-agent-codex'));

    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.output).toEqual(['Opening browser to sign in…', AUTHORIZE_LINE]);
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

    // The same check's login is joined, not refused: both entry points end up
    // following the one run.
    const joined = startAgentLogin('ai-agent-claude');
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);

    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBe('completed');
    await expect(joined).resolves.toBe('completed');
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

  it('cancels through the backend and ends the record without an error', async () => {
    const { agentLogin, agentLoginFor, cancelAgentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE));

    await cancelAgentLogin();
    expect(cancelDoctorLogin).toHaveBeenCalledWith('ai-agent-claude');
    // The request only asks; the end comes from the backend once the CLI is
    // dead, so the code box stays up until then — marked as on its way out.
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.cancelling).toBe(true);
    // A second click while that is pending asks nothing more.
    await cancelAgentLogin();
    expect(cancelDoctorLogin).toHaveBeenCalledTimes(1);

    registration.callback(done('ai-agent-claude', null, true));
    await expect(settled).resolves.toBe('cancelled');
    expect(agentLogin.running).toBe(false);
    expect(agentLogin.cancelling).toBe(false);
    expect(agentLogin.error).toBeNull();
    // A neutral end: nothing left for a UI to render, and nothing for the next
    // open to lead with.
    expect(agentLoginFor('ai-agent-claude')).toBeNull();
    expect(agentLogin.output).toEqual([]);
    expect(registration.unlisten).toHaveBeenCalled();
  });

  it('treats a cancel from another client as the same neutral end', async () => {
    const { agentLogin, agentLoginFor, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();

    // No cancel was asked for here; the `done` says the run was cancelled.
    registration.callback(done('ai-agent-claude', null, true));
    await expect(settled).resolves.toBe('cancelled');
    expect(cancelDoctorLogin).not.toHaveBeenCalled();
    expect(agentLogin.error).toBeNull();
    expect(agentLogin.running).toBe(false);
    expect(agentLoginFor('ai-agent-claude')).toBeNull();
  });

  it('re-syncs from the backend when a cancel finds nothing running', async () => {
    cancelDoctorLogin.mockResolvedValue(false);
    const { agentLogin, cancelAgentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    only().onEstablished?.();
    await flush();

    // The backend has no login for this check — its `done` never reached this
    // record — so the record must not stay `running` waiting for one.
    await cancelAgentLogin();
    expect(doctorLoginStatus).toHaveBeenCalledWith('ai-agent-claude');
    await expect(settled).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
    expect(agentLogin.cancelling).toBe(false);
  });

  it('re-attaches when the backend already has this login running, replaying its tail', async () => {
    startDoctorLogin.mockResolvedValue('alreadyRunning');
    doctorLoginStatus.mockResolvedValue(running(['Opening browser to sign in…', AUTHORIZE_LINE]));
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();

    // Not a failure: the CLI is alive and waiting for exactly the code this
    // record can send it. The listener stays, and the tail restores the URL.
    expect(doctorLoginStatus).toHaveBeenCalledWith('ai-agent-claude');
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.error).toBeNull();
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.output).toEqual(['Opening browser to sign in…', AUTHORIZE_LINE]);
    expect(registration.unlisten).not.toHaveBeenCalled();

    // Lines after the snapshot keep arriving live.
    registration.callback(line('ai-agent-claude', 'Paste code here if prompted >', 2));
    expect(agentLogin.output).toHaveLength(3);

    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBe('completed');
  });

  it('settles a re-attach whose login ended before the backend answered', async () => {
    startDoctorLogin.mockResolvedValue('alreadyRunning');
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    only().onEstablished?.();

    // Nothing running by the time the status answers, and no `done` left for
    // this listener: the record must not stay `running` for a dead process.
    await expect(settled).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
  });

  it('shows a line delivered while the snapshot was in flight exactly once', async () => {
    startDoctorLogin.mockResolvedValue('alreadyRunning');
    let answer!: (status: DoctorLoginStatus) => void;
    doctorLoginStatus.mockImplementation(
      () =>
        new Promise<DoctorLoginStatus>((resolve) => {
          answer = resolve;
        })
    );
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();
    expect(doctorLoginStatus).toHaveBeenCalled();

    // Two lines arrive live before the snapshot answers: seq 1 was recorded
    // before the snapshot was taken (so it is in it), seq 2 after.
    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE, 1));
    registration.callback(line('ai-agent-claude', 'Paste code here if prompted >', 2));
    answer({ running: true, output: ['Opening browser to sign in…', AUTHORIZE_LINE], nextSeq: 2 });
    await flush();

    expect(agentLogin.output).toEqual([
      'Opening browser to sign in…',
      AUTHORIZE_LINE,
      'Paste code here if prompted >',
    ]);
    expect(agentLogin.url).toBe('https://claude.ai/oauth');

    // A redelivery of a line already shown is dropped.
    registration.callback(line('ai-agent-claude', 'Paste code here if prompted >', 2));
    expect(agentLogin.output).toHaveLength(3);

    registration.callback(done('ai-agent-claude'));
    await settled;
  });

  it('attaches on open to a login the backend reports running', async () => {
    doctorLoginStatus.mockResolvedValue(running([AUTHORIZE_LINE]));
    const { agentLogin, attachAgentLogin } = await load();

    const attached = attachAgentLogin('ai-agent-claude');
    // The listener goes live before the backend is asked, so nothing the login
    // prints after the snapshot can be missed — and nothing is shown before the
    // backend has confirmed there is a login at all.
    const registration = only();
    expect(doctorLoginStatus).not.toHaveBeenCalled();
    expect(agentLogin.running).toBe(false);

    registration.onEstablished?.();
    await flush();
    expect(startDoctorLogin).not.toHaveBeenCalled();
    expect(agentLogin.checkId).toBe('ai-agent-claude');
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.output).toEqual([AUTHORIZE_LINE]);

    registration.callback(done('ai-agent-claude'));
    await expect(attached).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
  });

  it('leaves the record alone when there is nothing to attach to', async () => {
    const { agentLogin, attachAgentLogin } = await load();

    const attached = attachAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();

    await expect(attached).resolves.toBeNull();
    expect(agentLogin.checkId).toBeNull();
    expect(agentLogin.running).toBe(false);
    expect(registration.unlisten).toHaveBeenCalled();
  });

  it('lets a start take over an attach that has not been answered', async () => {
    let answer!: (status: DoctorLoginStatus) => void;
    doctorLoginStatus.mockImplementation(
      () =>
        new Promise<DoctorLoginStatus>((resolve) => {
          answer = resolve;
        })
    );
    const { agentLogin, attachAgentLogin, startAgentLogin } = await load();

    const attached = attachAgentLogin('ai-agent-claude');
    const probe = only();
    probe.onEstablished?.();

    // The user clicked `Log in` before the probe came back: the probe was only
    // a question, and the start answers it.
    const settled = startAgentLogin('ai-agent-claude');
    await expect(attached).resolves.toBeNull();
    expect(probe.unlisten).toHaveBeenCalled();
    const registration = only();
    registration.onEstablished?.();
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);

    // The probe's late answer has no record to write into.
    answer(running(['stale']));
    await flush();
    expect(agentLogin.output).toEqual([]);
    expect(agentLogin.running).toBe(true);

    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBe('completed');
  });

  it('re-syncs from the backend when the event channel reconnects', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();
    registration.callback(line('ai-agent-claude', 'Opening browser to sign in…'));

    // Two lines were emitted while the socket was down; the reconnect replays
    // them from the tail without showing the one already here twice.
    doctorLoginStatus.mockResolvedValue(
      running(['Opening browser to sign in…', AUTHORIZE_LINE, 'Paste code here if prompted >'])
    );
    registration.onEstablished?.();
    await flush();
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);
    expect(agentLogin.output).toEqual([
      'Opening browser to sign in…',
      AUTHORIZE_LINE,
      'Paste code here if prompted >',
    ]);
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.running).toBe(true);

    // A login that ended in the gap has no `done` left to deliver; the re-sync
    // settles it rather than leaving the record running forever.
    doctorLoginStatus.mockResolvedValue(idle());
    registration.onEstablished?.();
    await expect(settled).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
  });
});
