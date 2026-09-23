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
 * The record follows one *run*, identified by the run id the backend mints when
 * it claims the check's login slot and stamps on every event. The check id is
 * the slot, not the run: the backend releases the slot before a run's `done`
 * goes out, so a fresh start for the same check can be claimed between the two
 * and would otherwise take the earlier run's `done` as its own end — and a code
 * typed for the earlier run would go to the new one. Events from any other run
 * are dropped, and codes and cancels name the run. Events that arrive before
 * the backend has named the run (its answer to the start or the status is still
 * in flight) are held, then replayed once it has.
 *
 * The backend is the source of truth for whether a login is running, and this
 * record can lose it — a web client refreshed mid-login, a second client, a
 * reloaded webview. Both ways in re-attach rather than fail: a start the backend
 * answers "already running" adopts that run, and `attachAgentLogin` asks before
 * a UI offers to start one. Both replay the backend's output tail through
 * `doctorLoginStatus`, with the listener registered first; every line carries a
 * sequence number, per run, so a line delivered live while the snapshot was in
 * flight is shown once whichever arrived first. The record says which it did —
 * began the run, or picked up one already running (`origin`) — for a UI that
 * has to decide whether the login it shows is its own to end.
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
const LOGIN_OUTPUT_SUBSCRIPTION_ERROR = 'Could not subscribe to login output, try again';

/** How a login ended, short of failing. */
export type AgentLoginOutcome = 'completed' | 'cancelled';

/**
 * How this record came to follow the run it shows: `started` if this client's
 * `startAgentLogin` began it, `attached` if the run was already running when
 * this client picked it up — a start the backend answered "already running", or
 * an `attachAgentLogin` that found it.
 */
export type AgentLoginOrigin = 'started' | 'attached';

export interface AgentLoginState {
  /** Check id of the login in flight, or of the last one that ran. */
  checkId: string | null;
  /**
   * Run id of that login — the identity its events, codes and cancel go by.
   * Null until the backend has named the run it started or was found running.
   */
  runId: string | null;
  /**
   * Whether this client began that run or picked up one already running — see
   * `AgentLoginOrigin`. Null until the backend has answered the start, and
   * whenever nothing is followed. For a UI deciding whether the login it shows
   * is its own to end on the way out: a start it confirmed may have re-attached
   * to a login someone else is watching, and only the backend's answer says so.
   */
  origin: AgentLoginOrigin | null;
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
  runId: null,
  origin: null,
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
  agentLogin.runId = null;
  agentLogin.origin = null;
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
   * The run followed, once the backend has named it: the start's answer names
   * the run it began or found, the status snapshot the run it has. Until then
   * events for the check are held in `pending`.
   */
  runId: string | null;
  /**
   * Resolves with the run id once it is known, or with null if the attempt
   * ended first — for a code or a cancel asked for before the start answered.
   */
  runIdKnown: Promise<string | null>;
  resolveRunId: (runId: string | null) => void;
  /** Events received before `runId` was known, in arrival order. */
  pending: DoctorLoginOutput[];
  /**
   * Still asking the backend whether a login is running. Until it says so the
   * record is left alone: nothing may be running, and a UI must not show a
   * login that doesn't exist.
   */
  probing: boolean;
  /**
   * The backend has answered the start or the probe, so a re-sync on reconnect
   * has a run to ask about. Before that, a reconnect only records itself in
   * `gapBeforeAnswer` for the answer to act on.
   */
  answered: boolean;
  /**
   * The event channel reconnected while the answer was still in flight, so
   * events emitted in that gap were missed — possibly the run's first lines,
   * the sign-in URL among them. Either answer catches up from the backend when
   * this is set. A probe's answer is itself a snapshot, but not necessarily one
   * taken after the reconnect: in web mode the status is an HTTP fetch while
   * events ride the socket, so the backend can take the snapshot, the socket
   * can drop and come back, and the answer land last — with the lines emitted
   * between the snapshot and the reconnect in neither, and the next live line
   * moving `nextSeq` past them for good.
   */
  gapBeforeAnswer: boolean;
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
  let resolveRunId!: Attempt['resolveRunId'];
  const runIdKnown = new Promise<string | null>((res) => {
    resolveRunId = res;
  });
  const attempt: Attempt = {
    checkId,
    runId: null,
    runIdKnown,
    resolveRunId,
    pending: [],
    probing,
    answered: false,
    gapBeforeAnswer: false,
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
  attempt.pending = [];
  // A no-op if the run was named; otherwise releases a code or cancel that was
  // waiting for the name, which now has nothing to send to.
  attempt.resolveRunId(null);
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

function failRegistration(attempt: Attempt) {
  if (!live(attempt)) return;
  if (attempt.probing) {
    finish(attempt);
    attempt.reject(new Error(LOGIN_OUTPUT_SUBSCRIPTION_ERROR));
    return;
  }
  fail(attempt, LOGIN_OUTPUT_SUBSCRIPTION_ERROR);
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
 * The backend has named the run this attempt follows. From here on only that
 * run's events count; the ones held while the name was in flight are replayed
 * through the same filter, so an earlier run's late `done` in that window is
 * dropped and the followed run's own lines are shown.
 *
 * The replay can end the attempt (a held `done` of the followed run), so a
 * caller with more to do checks `live` afterwards.
 */
function adoptRun(attempt: Attempt, runId: string) {
  attempt.runId = runId;
  agentLogin.runId = runId;
  attempt.resolveRunId(runId);
  const held = attempt.pending;
  attempt.pending = [];
  for (const output of held) handleEvent(attempt, output);
}

/**
 * Bring a running attempt back in line with the backend: lines it missed are
 * replayed from the tail, and a login the backend no longer has is settled.
 *
 * "No longer has" includes a newer run holding the check's slot: the followed
 * run's `done` was missed, and the newer run is someone else's login. That
 * settle is `completed`, not because the login necessarily succeeded but
 * because its `done` is gone — it either passed before this client's listener
 * existed or was lost across a reconnect — and the record must not stay
 * `running` for a subprocess that has exited. Callers re-run the doctor checks
 * on `completed`, and the auth probe reports whether it actually signed in.
 */
async function syncFromBackend(attempt: Attempt) {
  const status = await doctorLoginStatus(attempt.checkId);
  if (!live(attempt)) return;
  if (!status.running || status.runId !== attempt.runId) {
    complete(attempt, 'completed');
    return;
  }
  applySnapshot(attempt, status);
}

/**
 * `syncFromBackend` for a run the backend has confirmed, tolerating a failed
 * status: the login is alive whether or not this client could ask about it, so
 * a rejected sync is logged and the run kept — the next event or reconnect
 * catches up — rather than reported as the login's failure.
 */
function catchUp(attempt: Attempt) {
  syncFromBackend(attempt).catch((e) => {
    console.warn(`[agentLogin] re-sync of ${attempt.checkId} failed:`, e);
  });
}

/** A reconnect of the event channel: events emitted in the gap were missed. */
function resync(attempt: Attempt) {
  if (!attempt.answered) {
    // No run to ask about yet. Recorded for the answer, which otherwise would
    // take the gap for a quiet stretch and never fetch what it dropped.
    attempt.gapBeforeAnswer = true;
    return;
  }
  catchUp(attempt);
}

function handleEvent(attempt: Attempt, output: DoctorLoginOutput) {
  // The check id is only a cheap pre-filter; the run id is the identity.
  if (!live(attempt) || output.checkId !== attempt.checkId) return;
  if (attempt.runId === null) {
    // Which run this attempt follows isn't known yet. Held rather than judged
    // by check id: this is exactly the window in which an earlier run's `done`
    // can arrive for the check. Bounded to what could ever be shown.
    attempt.pending = [...attempt.pending, output].slice(-MAX_OUTPUT_LINES);
    return;
  }
  if (output.runId !== attempt.runId) return;
  if (output.line !== null) applyLine(attempt, output.seq, output.line);
  if (!output.done) return;
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
 * from the backend, since events emitted in the gap were missed. A reconnect
 * that lands while the start's answer is still in flight can't ask yet — there
 * is no run id to ask about — so it is noted, and the answer does the asking.
 *
 * A start the backend answers "already running" is a re-attach, not a failure:
 * the CLI is alive and waiting for exactly the code this record can send it, so
 * its output so far is replayed and the record follows it to its end. Either
 * way the answer names the run, and the record follows that run alone.
 *
 * A start answered "started" with no gap asks nothing more: this listener was
 * live before the fix existed, so every line is on its way here — and a status
 * that found the run already over would settle it as `completed` ahead of a
 * `done` still in flight, trading a fast failure's error for a blank end.
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
      onRegistrationFailed: () => failRegistration(attempt),
      onEstablished: () => {
        if (!live(attempt)) return;
        if (started) {
          resync(attempt);
          return;
        }
        started = true;
        startDoctorLogin(checkId).then(
          (start) => {
            if (!live(attempt)) return;
            try {
              attempt.answered = true;
              // Which the backend says it did — began the run, or found one
              // already running — is what a UI reads to tell its own login
              // from one this click re-attached to.
              agentLogin.origin = start.outcome === 'alreadyRunning' ? 'attached' : 'started';
              adoptRun(attempt, start.runId);
              if (!live(attempt)) return;
              // A re-attach has the run's earlier output to fetch; a start only
              // has something to fetch if a reconnect while the answer was in
              // flight dropped lines. Tolerant of a failed status either way:
              // the backend has just confirmed the run is alive.
              if (start.outcome === 'alreadyRunning' || attempt.gapBeforeAnswer) {
                catchUp(attempt);
              }
            } catch (e) {
              // A throw in the answer's own handling — the held-events replay,
              // say — is not the start's failure, but left uncaught it would
              // escape as an unhandled rejection with the record still
              // `running` for a login nothing follows.
              if (live(attempt)) fail(attempt, errorText(e));
            }
          },
          (e) => {
            // The start itself was refused or never spawned. Only that is the
            // login's failure — hence the two-argument `then`, which keeps a
            // rejection in the catch-up above out of this handler.
            if (live(attempt)) fail(attempt, errorText(e));
          }
        );
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
 * running, leaving the record untouched; otherwise it takes the record, follows
 * the run the status names, and resolves like `startAgentLogin` when that run
 * ends.
 *
 * The listener is registered before the backend is asked, so nothing the login
 * prints after the snapshot can be missed; events that arrive while the snapshot
 * is in flight are held, then the run's own are merged by `seq` once it lands.
 * Unless the channel reconnected in the meantime — then the snapshot may predate
 * the gap, and the answer catches up from the backend as well.
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
      onRegistrationFailed: () => failRegistration(attempt),
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
            if (!status.running || status.runId === null) {
              abandon(attempt);
              return;
            }
            resetRecord(checkId, true);
            agentLogin.origin = 'attached';
            attempt.probing = false;
            adoptRun(attempt, status.runId);
            if (!live(attempt)) return;
            applySnapshot(attempt, status);
            // The snapshot is only current if the backend took it after the
            // channel's last reconnect, and a reconnect while it was in flight
            // says nothing about that (see `gapBeforeAnswer`). Asking again is
            // redundant when it was — the merge is by `seq` — and the backend
            // has just confirmed the run alive, so a failed catch-up is a
            // warning, not the login's failure.
            if (attempt.gapBeforeAnswer) catchUp(attempt);
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
 * The attempt that owns the record while a login is shown as running, if any.
 * A probe never owns it, and a settled attempt has let go.
 */
function runningAttempt(): Attempt | null {
  const attempt = current;
  if (!attempt || attempt.probing || !agentLogin.running) return null;
  return attempt;
}

/**
 * Send the typed code to the running login — to the run this record follows,
 * so a code typed for a login that has since ended is refused by the backend
 * (and the refusal shown) rather than delivered to a newer login for the check.
 * A send made before the backend has named the run waits for the name.
 *
 * The input stays open afterwards — see the module docs on why nothing in the
 * stream announces a re-prompt — so only the sent text is cleared.
 */
export async function submitAgentLoginCode(): Promise<void> {
  const attempt = runningAttempt();
  const code = agentLogin.code.trim();
  if (!attempt || !code || agentLogin.sending) return;
  agentLogin.sending = true;
  agentLogin.error = null;
  try {
    const runId = await attempt.runIdKnown;
    // Ended before it was named: its end is already on the record.
    if (runId === null || !live(attempt)) return;
    await sendDoctorLoginCode(attempt.checkId, runId, code);
    agentLogin.code = '';
  } catch (e) {
    agentLogin.error = errorText(e);
  } finally {
    agentLogin.sending = false;
  }
}

/**
 * Ask the backend to stop the running login — the run this record follows. The
 * end itself arrives as a `done` event with `cancelled` set, which settles the
 * record as a neutral end — no error, code box gone, record cleared — so
 * `cancelling` stays up until then. Idempotent while that is pending. A cancel
 * asked for before the backend has named the run waits for the name.
 *
 * A backend that reports the run not found has already lost the login this
 * record still shows — its `done` was missed, and any login now running for the
 * check is a newer one — so the record is re-synced from it instead of waiting
 * for an end that won't come.
 */
export async function cancelAgentLogin(): Promise<void> {
  const attempt = runningAttempt();
  if (!attempt || agentLogin.cancelling) return;
  agentLogin.cancelling = true;
  agentLogin.error = null;
  try {
    const runId = await attempt.runIdKnown;
    // Ended before it was named: its end is already on the record.
    if (runId === null || !live(attempt)) return;
    const cancelled = await cancelDoctorLogin(attempt.checkId, runId);
    if (!cancelled && live(attempt)) await syncFromBackend(attempt);
  } catch (e) {
    if (!live(attempt)) return;
    agentLogin.cancelling = false;
    agentLogin.error = errorText(e);
  }
}
