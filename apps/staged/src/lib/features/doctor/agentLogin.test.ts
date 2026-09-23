import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DoctorLoginOutput, DoctorLoginStart, DoctorLoginStatus } from '../../api/commands';

/** A registration made by the module under test through `listenToEvent`. */
interface Registration {
  event: string;
  callback: (payload: DoctorLoginOutput) => void;
  onEstablished?: () => void;
  unlisten: ReturnType<typeof vi.fn>;
}

const AUTHORIZE_LINE = 'If the browser didn’t open, visit: https://claude.ai/oauth';
/** The run the backend names unless a test says otherwise. */
const RUN = 'run-1';
/** An earlier run of the same check, whose events must not reach the record. */
const EARLIER_RUN = 'run-0';

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
    startDoctorLogin = vi.fn().mockResolvedValue(started());
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

  function started(runId = RUN): DoctorLoginStart {
    return { outcome: 'started', runId };
  }

  function alreadyRunning(runId = RUN): DoctorLoginStart {
    return { outcome: 'alreadyRunning', runId };
  }

  function line(
    checkId: string,
    text: string,
    seq: number = nextSeq++,
    runId = RUN
  ): DoctorLoginOutput {
    return { checkId, runId, line: text, seq, done: false, error: null, cancelled: false };
  }

  function done(
    checkId: string,
    error: string | null = null,
    cancelled = false,
    runId = RUN
  ): DoctorLoginOutput {
    return { checkId, runId, line: null, seq: nextSeq, done: true, error, cancelled };
  }

  function idle(): DoctorLoginStatus {
    return { running: false, runId: null, output: [], nextSeq: 0 };
  }

  function running(output: string[], runId = RUN): DoctorLoginStatus {
    return { running: true, runId, output, nextSeq: output.length };
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
    expect(agentLogin.runId).toBeNull();

    const registration = only();
    expect(registration.event).toBe('doctor-login-output');
    registration.onEstablished?.();
    expect(startDoctorLogin).toHaveBeenCalledWith('ai-agent-claude');
    await flush();
    // The start's answer names the run the record follows from here on.
    expect(agentLogin.runId).toBe(RUN);

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
    await flush();
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

  it('ignores a done for the same check from an earlier run', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();
    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE));

    // The earlier run of this check ended — with a failure, a cancel, and
    // plainly — after this one was started. None of those ends is this run's.
    registration.callback(done('ai-agent-claude', 'login failed', false, EARLIER_RUN));
    registration.callback(done('ai-agent-claude', null, true, EARLIER_RUN));
    registration.callback(done('ai-agent-claude', null, false, EARLIER_RUN));
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.error).toBeNull();
    expect(agentLogin.url).toBe('https://claude.ai/oauth');

    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBe('completed');
  });

  it('ignores output from an earlier run of the same check', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();

    // Sequence numbers are per run, so the earlier run's line 0 must not be
    // mistaken for this run's — neither shown nor counted against `nextSeq`.
    registration.callback(
      line('ai-agent-claude', 'visit: https://claude.ai/oauth/stale', 0, EARLIER_RUN)
    );
    expect(agentLogin.output).toEqual([]);
    expect(agentLogin.url).toBeNull();

    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE, 0));
    expect(agentLogin.output).toEqual([AUTHORIZE_LINE]);
    expect(agentLogin.url).toBe('https://claude.ai/oauth');

    registration.callback(done('ai-agent-claude'));
    await settled;
  });

  it('holds events that arrive before the start is answered and keeps only the new run’s', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    expect(agentLogin.runId).toBeNull();

    // The race this exists for: the earlier run released the check's slot, this
    // start claimed it, and only then did the earlier run's `done` go out — so
    // it lands here before the start's answer has named the new run. The new
    // run's first line is right behind it.
    registration.callback(done('ai-agent-claude', null, false, EARLIER_RUN));
    registration.callback(line('ai-agent-claude', 'Opening browser to sign in…', 0));
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.output).toEqual([]);

    await flush();
    expect(agentLogin.runId).toBe(RUN);
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.output).toEqual(['Opening browser to sign in…']);

    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE, 1));
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBe('completed');
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
    // Sent to the run the start named, not just the check.
    expect(sendDoctorLoginCode).toHaveBeenCalledWith('ai-agent-claude', RUN, 'abc-123');
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

  it('refuses a code for a run the backend has replaced, and settles on the cancel', async () => {
    const refusal =
      'The login this code was typed for has ended; a newer login is running for ' +
      'ai-agent-claude and the code was not delivered to it';
    sendDoctorLoginCode.mockRejectedValue(new Error(refusal));
    // This record's run ended and its `done` was missed; another client has
    // since started a new run for the same check.
    cancelDoctorLogin.mockResolvedValue(false);
    doctorLoginStatus.mockResolvedValue(running([AUTHORIZE_LINE], 'run-2'));
    const { agentLogin, cancelAgentLogin, startAgentLogin, submitAgentLoginCode } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    only().onEstablished?.();
    await flush();
    expect(agentLogin.runId).toBe(RUN);

    // The code names this record's run, so the backend refuses rather than
    // handing it to the newer login — and the refusal is what the user sees.
    agentLogin.code = 'abc-123';
    await submitAgentLoginCode();
    expect(sendDoctorLoginCode).toHaveBeenCalledWith('ai-agent-claude', RUN, 'abc-123');
    expect(agentLogin.error).toBe(refusal);
    expect(agentLogin.code).toBe('abc-123');
    expect(agentLogin.running).toBe(true);

    // Cancelling names the run too: the newer login is left alone, and the
    // backend not finding this run is the cue to re-sync — which finds a
    // different run holding the slot and settles this one.
    await cancelAgentLogin();
    expect(cancelDoctorLogin).toHaveBeenCalledWith('ai-agent-claude', RUN);
    await expect(settled).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
    expect(agentLogin.cancelling).toBe(false);
    // The newer run's tail was not adopted: it is someone else's login.
    expect(agentLogin.output).toEqual([]);
    expect(agentLogin.runId).toBe(RUN);
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
    expect(agentLogin.runId).toBeNull();
  });

  it('cancels through the backend and ends the record without an error', async () => {
    const { agentLogin, agentLoginFor, cancelAgentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE));

    await cancelAgentLogin();
    expect(cancelDoctorLogin).toHaveBeenCalledWith('ai-agent-claude', RUN);
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
    expect(agentLogin.runId).toBeNull();
    expect(agentLogin.origin).toBeNull();
    expect(registration.unlisten).toHaveBeenCalled();
  });

  it('records whether the start began the run or re-attached to one already running', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    // Nothing is claimed until the backend answers: a start may yet turn out to
    // be a re-attach, and a UI that assumed otherwise would end a login someone
    // else is watching on its way out.
    expect(agentLogin.origin).toBeNull();
    registration.onEstablished?.();
    await flush();
    expect(agentLogin.origin).toBe('started');
    registration.callback(done('ai-agent-claude'));
    await settled;
    // A finished login keeps its origin, like the rest of what it left behind.
    expect(agentLogin.origin).toBe('started');

    // The same start, answered "already running": a login this client did not
    // begin, so not its caller's to end.
    startDoctorLogin.mockResolvedValue(alreadyRunning('run-7'));
    doctorLoginStatus.mockResolvedValue(running([AUTHORIZE_LINE], 'run-7'));
    const reattached = startAgentLogin('ai-agent-claude');
    const again = only();
    expect(agentLogin.origin).toBeNull();
    again.onEstablished?.();
    await flush();
    expect(agentLogin.origin).toBe('attached');
    expect(agentLogin.runId).toBe('run-7');
    expect(agentLogin.running).toBe(true);

    again.callback(done('ai-agent-claude', null, false, 'run-7'));
    await expect(reattached).resolves.toBe('completed');
  });

  it('reports a throw in the start answer’s own handling as the login’s failure', async () => {
    // Fault injection: nothing on that path throws today, so the record itself
    // is made to refuse the run id the answer hands it.
    vi.stubGlobal(
      '$state',
      (initial: object) =>
        new Proxy(initial, {
          set(target, key, value) {
            if (key === 'runId' && value === RUN) throw new Error('record refused the run');
            return Reflect.set(target, key, value);
          },
        })
    );
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    only().onEstablished?.();

    // Left uncaught this escaped `then` as an unhandled rejection, with the
    // record still `running` for a login nothing was following.
    await expect(settled).rejects.toThrow('record refused the run');
    expect(agentLogin.running).toBe(false);
    expect(agentLogin.error).toBe('record refused the run');
  });

  it('waits for the run to be named before cancelling it', async () => {
    let answer!: (start: DoctorLoginStart) => void;
    startDoctorLogin.mockImplementation(
      () =>
        new Promise<DoctorLoginStart>((resolve) => {
          answer = resolve;
        })
    );
    const { agentLogin, cancelAgentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();

    // Cancel clicked before the start answered: there is no run id to name yet,
    // and cancelling "whatever is running for the check" could hit another
    // client's newer login. The cancel waits for the name instead of being
    // dropped — a dropped cancel would leave the CLI holding the slot until
    // doctor's fix timeout.
    const cancelling = cancelAgentLogin();
    await flush();
    expect(agentLogin.cancelling).toBe(true);
    expect(cancelDoctorLogin).not.toHaveBeenCalled();

    answer(started());
    await cancelling;
    expect(cancelDoctorLogin).toHaveBeenCalledWith('ai-agent-claude', RUN);

    registration.callback(done('ai-agent-claude', null, true));
    await expect(settled).resolves.toBe('cancelled');
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
    startDoctorLogin.mockResolvedValue(alreadyRunning('run-7'));
    doctorLoginStatus.mockResolvedValue(
      running(['Opening browser to sign in…', AUTHORIZE_LINE], 'run-7')
    );
    const { agentLogin, startAgentLogin, submitAgentLoginCode } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();

    // Not a failure: the CLI is alive and waiting for exactly the code this
    // record can send it. The listener stays, the record takes the run id the
    // backend named, and the tail restores the URL.
    expect(doctorLoginStatus).toHaveBeenCalledWith('ai-agent-claude');
    expect(agentLogin.runId).toBe('run-7');
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.error).toBeNull();
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.output).toEqual(['Opening browser to sign in…', AUTHORIZE_LINE]);
    expect(registration.unlisten).not.toHaveBeenCalled();

    // Lines after the snapshot keep arriving live — under that run id.
    registration.callback(line('ai-agent-claude', 'Paste code here if prompted >', 2, 'run-7'));
    expect(agentLogin.output).toHaveLength(3);
    // And a code goes to that run.
    agentLogin.code = 'abc-123';
    await submitAgentLoginCode();
    expect(sendDoctorLoginCode).toHaveBeenCalledWith('ai-agent-claude', 'run-7', 'abc-123');

    registration.callback(done('ai-agent-claude', null, false, 'run-7'));
    await expect(settled).resolves.toBe('completed');
  });

  it('settles a re-attach whose login ended before the backend answered', async () => {
    startDoctorLogin.mockResolvedValue(alreadyRunning());
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    only().onEstablished?.();

    // Nothing running by the time the status answers, and no `done` left for
    // this listener: the record must not stay `running` for a dead process.
    await expect(settled).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
  });

  it('settles a re-attach whose run the backend has since replaced', async () => {
    startDoctorLogin.mockResolvedValue(alreadyRunning());
    // Between the start's answer and the status, that run ended and another
    // client started a new one for the check.
    doctorLoginStatus.mockResolvedValue(running([AUTHORIZE_LINE], 'run-2'));
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    only().onEstablished?.();

    // The run this record was attached to is gone, and the one running is not
    // this record's to show: its `done` was missed, so it is settled.
    await expect(settled).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
    expect(agentLogin.output).toEqual([]);
  });

  it('shows a line delivered while the snapshot was in flight exactly once', async () => {
    startDoctorLogin.mockResolvedValue(alreadyRunning());
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
    answer({
      running: true,
      runId: RUN,
      output: ['Opening browser to sign in…', AUTHORIZE_LINE],
      nextSeq: 2,
    });
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
    doctorLoginStatus.mockResolvedValue(running([AUTHORIZE_LINE], 'run-7'));
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
    // The status names the run; the record follows it, as a run this client
    // picked up rather than began.
    expect(agentLogin.runId).toBe('run-7');
    expect(agentLogin.origin).toBe('attached');
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.output).toEqual([AUTHORIZE_LINE]);
    // No reconnect while the status was in flight, so the snapshot is current
    // and nothing more is asked.
    expect(doctorLoginStatus).toHaveBeenCalledTimes(1);

    // An earlier run's late `done` for the check is not this run's end.
    registration.callback(done('ai-agent-claude', null, false, EARLIER_RUN));
    expect(agentLogin.running).toBe(true);

    registration.callback(done('ai-agent-claude', null, false, 'run-7'));
    await expect(attached).resolves.toBe('completed');
    expect(agentLogin.running).toBe(false);
  });

  it('catches up after the probe when the channel reconnected while the status was in flight', async () => {
    let answer!: (status: DoctorLoginStatus) => void;
    doctorLoginStatus.mockImplementationOnce(
      () =>
        new Promise<DoctorLoginStatus>((resolve) => {
          answer = resolve;
        })
    );
    const { agentLogin, attachAgentLogin } = await load();

    const attached = attachAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    expect(doctorLoginStatus).toHaveBeenCalledTimes(1);

    // The socket dropped and came back while the status was in flight. In web
    // mode the status is an HTTP fetch and events ride the socket, so the
    // backend may have taken its snapshot before the drop: a line the login
    // printed between the two is in neither, and the next live line would move
    // `nextSeq` past it for good. Nothing can be asked yet — no run is named.
    registration.onEstablished?.();
    await flush();
    expect(doctorLoginStatus).toHaveBeenCalledTimes(1);
    expect(agentLogin.running).toBe(false);

    // The snapshot from before the gap lands; the gap makes the answer ask
    // again, and the second snapshot has the line the first predates.
    doctorLoginStatus.mockResolvedValue(running(['Opening browser to sign in…', AUTHORIZE_LINE]));
    answer(running(['Opening browser to sign in…']));
    await flush();
    expect(doctorLoginStatus).toHaveBeenCalledTimes(2);
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.runId).toBe(RUN);
    expect(agentLogin.output).toEqual(['Opening browser to sign in…', AUTHORIZE_LINE]);
    expect(agentLogin.url).toBe('https://claude.ai/oauth');

    // Lines after that keep arriving live, merged by `seq`.
    registration.callback(line('ai-agent-claude', 'Paste code here if prompted >', 2));
    expect(agentLogin.output).toHaveLength(3);

    registration.callback(done('ai-agent-claude'));
    await expect(attached).resolves.toBe('completed');
  });

  it('holds events that arrive during the probe and keeps only the found run’s', async () => {
    let answer!: (status: DoctorLoginStatus) => void;
    doctorLoginStatus.mockImplementation(
      () =>
        new Promise<DoctorLoginStatus>((resolve) => {
          answer = resolve;
        })
    );
    const { agentLogin, attachAgentLogin } = await load();

    const attached = attachAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();

    // While the status is in flight: an earlier run's `done`, then a line of
    // the run the status is about to name that the snapshot won't cover.
    registration.callback(done('ai-agent-claude', null, false, EARLIER_RUN));
    registration.callback(line('ai-agent-claude', 'Paste code here if prompted >', 1));
    expect(agentLogin.running).toBe(false);

    answer({ running: true, runId: RUN, output: [AUTHORIZE_LINE], nextSeq: 1 });
    await flush();
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.runId).toBe(RUN);
    expect(agentLogin.output).toEqual([AUTHORIZE_LINE, 'Paste code here if prompted >']);

    registration.callback(done('ai-agent-claude'));
    await expect(attached).resolves.toBe('completed');
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

  it('catches up from the backend when the channel reconnected before the start was answered', async () => {
    let answer!: (start: DoctorLoginStart) => void;
    startDoctorLogin.mockImplementation(
      () =>
        new Promise<DoctorLoginStart>((resolve) => {
          answer = resolve;
        })
    );
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);

    // The socket dropped and came back while the start's answer was in flight.
    // There is no run id to ask about yet, so nothing can be fetched here —
    // but the fix printed its first lines, the sign-in URL among them, into
    // that gap.
    registration.onEstablished?.();
    await flush();
    expect(startDoctorLogin).toHaveBeenCalledTimes(1);
    expect(doctorLoginStatus).not.toHaveBeenCalled();

    // The answer names the run; the gap is what makes it ask the backend.
    doctorLoginStatus.mockResolvedValue(running(['Opening browser to sign in…', AUTHORIZE_LINE]));
    answer(started());
    await flush();
    expect(doctorLoginStatus).toHaveBeenCalledWith('ai-agent-claude');
    expect(agentLogin.runId).toBe(RUN);
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    expect(agentLogin.output).toEqual(['Opening browser to sign in…', AUTHORIZE_LINE]);

    // Lines after the snapshot keep arriving live, merged by `seq`.
    registration.callback(line('ai-agent-claude', 'Paste code here if prompted >', 2));
    expect(agentLogin.output).toHaveLength(3);

    registration.callback(done('ai-agent-claude'));
    await expect(settled).resolves.toBe('completed');
  });

  it('asks nothing after a start answered without a gap, so a fast failure keeps its error', async () => {
    const { agentLogin, startAgentLogin } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();
    expect(agentLogin.runId).toBe(RUN);

    // The listener was live before the fix existed, so every line is on its
    // way here and there is nothing to fetch. Asking anyway would race the
    // fix's own end: the status mock answers "not running", and a sync would
    // settle the record as `completed` — swallowing the `done` behind it and
    // the error it carries.
    expect(doctorLoginStatus).not.toHaveBeenCalled();
    registration.callback(done('ai-agent-claude', 'spawn failed: zsh: command not found'));
    await expect(settled).rejects.toThrow('spawn failed: zsh: command not found');
    expect(agentLogin.error).toBe('spawn failed: zsh: command not found');
    expect(agentLogin.running).toBe(false);
  });

  it('keeps following a re-attached login when the catch-up status fails', async () => {
    startDoctorLogin.mockResolvedValue(alreadyRunning('run-7'));
    doctorLoginStatus.mockRejectedValue(new Error('IPC channel closed'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { agentLogin, startAgentLogin, submitAgentLoginCode } = await load();

    const settled = startAgentLogin('ai-agent-claude');
    const registration = only();
    registration.onEstablished?.();
    await flush();

    // The backend has just confirmed the login is alive; a status hiccup is not
    // its failure. The record keeps the run and the listener, and only the
    // tail is missing until the next event or reconnect.
    expect(doctorLoginStatus).toHaveBeenCalledWith('ai-agent-claude');
    expect(warn).toHaveBeenCalled();
    expect(agentLogin.running).toBe(true);
    expect(agentLogin.runId).toBe('run-7');
    expect(agentLogin.error).toBeNull();
    expect(agentLogin.output).toEqual([]);
    expect(registration.unlisten).not.toHaveBeenCalled();

    // Still very much a login: lines show and a code goes to the run.
    registration.callback(line('ai-agent-claude', AUTHORIZE_LINE, 1, 'run-7'));
    expect(agentLogin.url).toBe('https://claude.ai/oauth');
    agentLogin.code = 'abc-123';
    await submitAgentLoginCode();
    expect(sendDoctorLoginCode).toHaveBeenCalledWith('ai-agent-claude', 'run-7', 'abc-123');

    // The next reconnect fetches what the failed catch-up could not.
    doctorLoginStatus.mockResolvedValue(
      running(['Opening browser to sign in…', AUTHORIZE_LINE], 'run-7')
    );
    registration.onEstablished?.();
    await flush();
    expect(agentLogin.output).toEqual(['Opening browser to sign in…', AUTHORIZE_LINE]);

    registration.callback(done('ai-agent-claude', null, false, 'run-7'));
    await expect(settled).resolves.toBe('completed');
    warn.mockRestore();
  });
});
