/**
 * agentLogin.svelte.ts — shared state for an interactive agent CLI login.
 *
 * A login is a streamed subprocess, not a request/response: the CLI prints a
 * verification URL, tries to open a browser, and then blocks reading the code
 * the sign-in page hands back. Doctor's fix runner gives it a piped stdin and
 * streams its output as `doctor-login-output` events, and allows exactly one
 * login per check id — so the two entry points that can start one (the session
 * pane's authentication alert and the Doctor panel's `Fix` on an `auth` check)
 * share the single record here rather than each keeping their own.
 *
 * The code entry is deliberately *not* gated on detecting a prompt. Claude's
 * prompt is `Paste code here if prompted > ` with no trailing newline, and
 * doctor's reader splits the fix's output into lines, so the prompt is only
 * delivered once stdout closes at exit — far too late to trigger anything. The
 * input is offered for as long as the login runs instead, which also leaves a
 * mistyped code a way to be retried: the CLI re-prompts (again with no newline)
 * on a code it rejects locally.
 *
 * The backend is the source of truth for whether a login is running, and this
 * record can lose it — a web client refreshed mid-login, a second client, a
 * reloaded webview. Both ways in re-attach rather than fail: a start the backend
 * answers "already running" adopts that run, and `attachAgentLogin` asks before
 * a UI offers to start one. Both replay the backend's output tail through
 * `doctorLoginStatus`, with the listener registered first; every line carries a
 * sequence number, so a line delivered live while the snapshot was in flight is
 * shown once whichever arrived first.
 *
 * Ending a login early is `cancelAgentLogin`, which goes through doctor's
 * cancellation token and kills the CLI. Closing its stdin would not do: the CLI
 * ignores EOF once it is waiting on its browser callback.
 */
import {
  cancelDoctorLogin,
  doctorLoginStatus,
  sendDoctorLoginCode,
  startDoctorLogin,
  type DoctorLoginOutput,
  type DoctorLoginStatus,
} from '../../api/commands';
import { listenToEvent, type UnlistenFn } from '../../transport';

/** Output lines kept for display, oldest first. */
const MAX_OUTPUT_LINES = 40;

/** How a login ended, short of failing. */
export type AgentLoginOutcome = 'completed' | 'cancelled';

export interface AgentLoginState {
  /** Check id of the login in flight, or of the last one that ran. */
  checkId: string | null;
  running: boolean;
  /**
   * The sign-in URL the CLI printed, when it printed one. The whole point of
   * showing it: the CLI's own `open` can fail silently inside the Tauri
   * process, and a user driving Staged through the web server never had a
   * browser on the host at all.
   */
  url: string | null;
  /** Tail of the fix's output, so the user can see what it is waiting on. */
  output: string[];
  /** Failure of the fix itself, of starting it, or of sending a code. */
  error: string | null;
  /** The code being typed. Kept here so it survives the pane re-rendering. */
  code: string;
  sending: boolean;
  /** A cancel has been asked for and the backend has yet to report the end. */
  cancelling: boolean;
}

export const agentLogin: AgentLoginState = $state({
  checkId: null,
  running: false,
  url: null,
  output: [],
  error: null,
  code: '',
  sending: false,
  cancelling: false,
});

/** The shared record, but only when it describes `checkId`'s login. */
export function agentLoginFor(checkId: string | null | undefined): AgentLoginState | null {
  if (!checkId || agentLogin.checkId !== checkId) return null;
  return agentLogin;
}

/**
 * The sign-in URL carried by a login output line, if any.
 *
 * The first URL in the run wins: the CLIs print the authorize URL before
 * anything else (`If the browser didn't open, visit: <url>`), so a later one is
 * more likely to be a docs or support link than a better address.
 */
export function extractLoginUrl(line: string): string | null {
  const match = /https?:\/\/[^\s<>"'`]+/.exec(line);
  if (!match) return null;
  // Trailing sentence punctuation is not part of the URL.
  return match[0].replace(/[.,;:!)\]]+$/, '');
}

function resetRecord(checkId: string | null, running: boolean) {
  agentLogin.checkId = checkId;
  agentLogin.running = running;
  agentLogin.url = null;
  agentLogin.output = [];
  agentLogin.error = null;
  agentLogin.code = '';
  agentLogin.sending = false;
  agentLogin.cancelling = false;
}

/**
 * Discard what a finished login left behind, so a UI that opens on `checkId`
 * again doesn't lead with the last attempt's error. A running login is never
 * cleared — it is still writing to the record.
 */
export function clearAgentLogin(checkId: string) {
  if (agentLogin.running || agentLogin.checkId !== checkId) return;
  resetRecord(null, false);
}

interface OutputLine {
  seq: number;
  text: string;
}

/**
 * One start or attach, and the run it follows. Every continuation — the start's
 * answer, a status snapshot, an event — checks that its attempt is still the
 * current one before touching the record, so an attempt superseded by a newer
 * one can't write into it.
 */
interface Attempt {
  checkId: string;
  /**
   * Still asking the backend whether a login is running. Until it says so the
   * record is left alone: nothing may be running, and a UI must not show a
   * login that doesn't exist.
   */
  probing: boolean;
  /**
   * The backend has answered the start or the probe, so a re-sync on reconnect
   * has a run to ask about. Before that the answer still to come covers it.
   */
  answered: boolean;
  /** The lines shown, with the `seq` each arrived under. */
  lines: OutputLine[];
  /** `seq` of the next line expected; one below it was already shown or replayed. */
  nextSeq: number;
  settled: boolean;
  promise: Promise<AgentLoginOutcome | null>;
  resolve: (outcome: AgentLoginOutcome | null) => void;
  reject: (error: Error) => void;
}

let unlisten: UnlistenFn | null = null;
/** The attempt in flight, if any — owner of the record and the listener. */
let current: Attempt | null = null;

function stopWatching() {
  unlisten?.();
  unlisten = null;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function newAttempt(checkId: string, probing: boolean): Attempt {
  let resolve!: Attempt['resolve'];
  let reject!: Attempt['reject'];
  const promise = new Promise<AgentLoginOutcome | null>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  const attempt: Attempt = {
    checkId,
    probing,
    answered: false,
    lines: [],
    nextSeq: 0,
    settled: false,
    promise,
    resolve,
    reject,
  };
  current = attempt;
  return attempt;
}

/** Whether `attempt` still owns the record and the listener. */
function live(attempt: Attempt): boolean {
  return current === attempt && !attempt.settled;
}

/** Give up the record and the listener. Every end goes through here. */
function finish(attempt: Attempt) {
  attempt.settled = true;
  stopWatching();
  if (current === attempt) current = null;
}

function complete(attempt: Attempt, outcome: AgentLoginOutcome) {
  finish(attempt);
  if (outcome === 'cancelled') {
    // A neutral end: nothing to render, no error to lead with next time.
    resetRecord(null, false);
  } else {
    agentLogin.running = false;
    agentLogin.sending = false;
    agentLogin.cancelling = false;
  }
  attempt.resolve(outcome);
}

function fail(attempt: Attempt, error: string) {
  finish(attempt);
  agentLogin.running = false;
  agentLogin.sending = false;
  agentLogin.cancelling = false;
  agentLogin.error = error;
  attempt.reject(new Error(error));
}

/** A probe that found nothing — or was superseded — leaves the record alone. */
function abandon(attempt: Attempt) {
  finish(attempt);
  attempt.resolve(null);
}

function render(attempt: Attempt) {
  agentLogin.output = attempt.lines.map((line) => line.text);
  if (agentLogin.url) return;
  for (const line of attempt.lines) {
    const url = extractLoginUrl(line.text);
    if (url) {
      agentLogin.url = url;
      return;
    }
  }
}

/**
 * Show a line delivered live — unless it, or a later one, was already shown:
 * a redelivery, or a line a status snapshot had covered.
 */
function applyLine(attempt: Attempt, seq: number, text: string) {
  if (seq < attempt.nextSeq) return;
  attempt.nextSeq = seq + 1;
  attempt.lines = [...attempt.lines, { seq, text }].slice(-MAX_OUTPUT_LINES);
  if (!attempt.probing) render(attempt);
}

/**
 * Replace what is shown with the backend's snapshot, keeping the lines that
 * arrived live after it was taken. The snapshot's `output` covers the `seq`s
 * from `nextSeq - output.length` up to `nextSeq`: a live line below that is
 * either in it or older than the tail, and one at or above it came later.
 */
function applySnapshot(attempt: Attempt, status: DoctorLoginStatus) {
  const firstSeq = status.nextSeq - status.output.length;
  const replayed = status.output.map((text, i) => ({ seq: firstSeq + i, text }));
  const since = attempt.lines.filter((line) => line.seq >= status.nextSeq);
  attempt.lines = [...replayed, ...since].slice(-MAX_OUTPUT_LINES);
  attempt.nextSeq = Math.max(attempt.nextSeq, status.nextSeq);
  render(attempt);
}

/**
 * Bring a running attempt back in line with the backend: lines it missed are
 * replayed from the tail, and a login the backend no longer has is settled.
 *
 * That settle is `completed`, not because the login necessarily succeeded but
 * because its `done` is gone — it either passed before this client's listener
 * existed or was lost across a reconnect — and the record must not stay
 * `running` for a subprocess that has exited. Callers re-run the doctor checks
 * on `completed`, and the auth probe reports whether it actually signed in.
 */
async function syncFromBackend(attempt: Attempt) {
  const status = await doctorLoginStatus(attempt.checkId);
  if (!live(attempt)) return;
  if (!status.running) {
    complete(attempt, 'completed');
    return;
  }
  applySnapshot(attempt, status);
}

/** A reconnect of the event channel: events emitted in the gap were missed. */
function resync(attempt: Attempt) {
  if (!attempt.answered) return;
  syncFromBackend(attempt).catch((e) => {
    // Nothing to do but wait for the next event or reconnect; the record still
    // describes a login the backend confirmed.
    console.warn(`[agentLogin] re-sync of ${attempt.checkId} failed:`, e);
  });
}

function handleEvent(attempt: Attempt, output: DoctorLoginOutput) {
  if (!live(attempt) || output.checkId !== attempt.checkId) return;
  if (output.line !== null) applyLine(attempt, output.seq, output.line);
  if (!output.done) return;
  if (attempt.probing) {
    // Ended before the backend confirmed it was running: this client never
    // showed the login, so there is nothing to end.
    abandon(attempt);
    return;
  }
  if (output.cancelled) complete(attempt, 'cancelled');
  else if (output.error !== null) fail(attempt, output.error);
  else complete(attempt, 'completed');
}

/**
 * Start a login for `checkId`, resolving with how it ended and rejecting with
 * its failure (which is also left on the shared record for the UI to render).
 *
 * The fix is started from `onEstablished`, not beside the `listenToEvent` call:
 * registration is asynchronous, and a login that fails to spawn emits its
 * `done` event immediately — lost in that gap, it would leave `running` true
 * with no way back. `onEstablished` also fires on every web-socket reconnect:
 * the start is latched to the first one, and each later one re-syncs the record
 * from the backend, since events emitted in the gap were missed.
 *
 * A start the backend answers "already running" is a re-attach, not a failure:
 * the CLI is alive and waiting for exactly the code this record can send it, so
 * its output so far is replayed and the record follows it to its end.
 */
export function startAgentLogin(checkId: string): Promise<AgentLoginOutcome> {
  if (current) {
    if (current.probing) {
      // A probe is only a question; the start answers it.
      abandon(current);
    } else if (current.checkId === checkId) {
      // Same login: follow the one that is running rather than start another
      // the backend would refuse.
      return current.promise.then(asOutcome);
    } else {
      // One record, one login: taking it over would strand the running fix's
      // awaiter and hide the record it is still writing to.
      return Promise.reject(new Error(`A login is already running for ${current.checkId}`));
    }
  }
  stopWatching();
  resetRecord(checkId, true);
  const attempt = newAttempt(checkId, false);

  let started = false;
  unlisten = listenToEvent<DoctorLoginOutput>(
    'doctor-login-output',
    (output) => handleEvent(attempt, output),
    {
      onEstablished: () => {
        if (!live(attempt)) return;
        if (started) {
          resync(attempt);
          return;
        }
        started = true;
        startDoctorLogin(checkId)
          .then(async (start) => {
            if (!live(attempt)) return;
            attempt.answered = true;
            if (start === 'alreadyRunning') await syncFromBackend(attempt);
          })
          .catch((e) => {
            if (live(attempt)) fail(attempt, errorText(e));
          });
      },
    }
  );
  return attempt.promise.then(asOutcome);
}

/** A start never resolves `null` — only a probe does; this satisfies the type. */
function asOutcome(outcome: AgentLoginOutcome | null): AgentLoginOutcome {
  return outcome ?? 'completed';
}

/**
 * Pick up a login for `checkId` that is already running on the backend — one
 * started from the other entry point, from another client, or before this view
 * reloaded — so its URL and code box come back instead of a button the backend
 * would answer "already running". Resolves `null` straight away when nothing is
 * running, leaving the record untouched; otherwise it takes the record and
 * resolves like `startAgentLogin` when the login ends.
 *
 * The listener is registered before the backend is asked, so nothing the login
 * prints after the snapshot can be missed; lines that arrive while the snapshot
 * is in flight are held and merged by `seq` once it lands.
 */
export function attachAgentLogin(checkId: string): Promise<AgentLoginOutcome | null> {
  if (current) {
    // A run or a probe for this check is already being followed.
    if (current.checkId === checkId) return current.promise;
    // Another check's login owns the record.
    if (!current.probing) return Promise.resolve(null);
    // A probe for another check that hasn't answered yet: the latest ask wins.
    abandon(current);
  }
  stopWatching();
  const attempt = newAttempt(checkId, true);

  let asked = false;
  unlisten = listenToEvent<DoctorLoginOutput>(
    'doctor-login-output',
    (output) => handleEvent(attempt, output),
    {
      onEstablished: () => {
        if (!live(attempt)) return;
        if (asked) {
          resync(attempt);
          return;
        }
        asked = true;
        doctorLoginStatus(checkId)
          .then((status) => {
            if (!live(attempt)) return;
            attempt.answered = true;
            if (!status.running) {
              abandon(attempt);
              return;
            }
            resetRecord(checkId, true);
            attempt.probing = false;
            applySnapshot(attempt, status);
          })
          .catch((e) => {
            if (!live(attempt)) return;
            finish(attempt);
            attempt.reject(new Error(errorText(e)));
          });
      },
    }
  );
  return attempt.promise;
}

/**
 * Send the typed code to the running login.
 *
 * The input stays open afterwards — see the module docs on why nothing in the
 * stream announces a re-prompt — so only the sent text is cleared.
 */
export async function submitAgentLoginCode(): Promise<void> {
  const checkId = agentLogin.checkId;
  const code = agentLogin.code.trim();
  if (!checkId || !agentLogin.running || !code || agentLogin.sending) return;
  agentLogin.sending = true;
  agentLogin.error = null;
  try {
    await sendDoctorLoginCode(checkId, code);
    agentLogin.code = '';
  } catch (e) {
    agentLogin.error = errorText(e);
  } finally {
    agentLogin.sending = false;
  }
}

/**
 * Ask the backend to stop the running login. The end itself arrives as a
 * `done` event with `cancelled` set, which settles the record as a neutral end
 * — no error, code box gone, record cleared — so `cancelling` stays up until
 * then. Idempotent while that is pending.
 *
 * A backend that reports nothing running has already lost the login this
 * record still shows — its `done` was missed — so the record is re-synced
 * from it instead of waiting for an end that won't come.
 */
export async function cancelAgentLogin(): Promise<void> {
  const attempt = current;
  const checkId = agentLogin.checkId;
  if (!attempt || !checkId || !agentLogin.running || agentLogin.cancelling) return;
  agentLogin.cancelling = true;
  agentLogin.error = null;
  try {
    const cancelled = await cancelDoctorLogin(checkId);
    if (!cancelled && live(attempt)) await syncFromBackend(attempt);
  } catch (e) {
    if (!live(attempt)) return;
    agentLogin.cancelling = false;
    agentLogin.error = errorText(e);
  }
}
