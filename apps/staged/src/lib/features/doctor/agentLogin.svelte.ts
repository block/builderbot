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
 */
import { sendDoctorLoginCode, startDoctorLogin, type DoctorLoginOutput } from '../../api/commands';
import { listenToEvent, type UnlistenFn } from '../../transport';

/** Output lines kept for display, oldest first. */
const MAX_OUTPUT_LINES = 40;

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
}

export const agentLogin: AgentLoginState = $state({
  checkId: null,
  running: false,
  url: null,
  output: [],
  error: null,
  code: '',
  sending: false,
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

/**
 * Discard what a finished login left behind, so a UI that opens on `checkId`
 * again doesn't lead with the last attempt's error. A running login is never
 * cleared — it is still writing to the record.
 */
export function clearAgentLogin(checkId: string) {
  if (agentLogin.running || agentLogin.checkId !== checkId) return;
  agentLogin.checkId = null;
  agentLogin.url = null;
  agentLogin.output = [];
  agentLogin.error = null;
  agentLogin.code = '';
}

let unlisten: UnlistenFn | null = null;

function stopWatching() {
  unlisten?.();
  unlisten = null;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function recordLine(line: string) {
  agentLogin.output = [...agentLogin.output, line].slice(-MAX_OUTPUT_LINES);
  if (!agentLogin.url) agentLogin.url = extractLoginUrl(line);
}

/**
 * Start a login for `checkId`, resolving when it finishes and rejecting with
 * its failure (which is also left on the shared record for the UI to render).
 *
 * The fix is started from `onEstablished`, not beside the `listenToEvent` call:
 * registration is asynchronous, and a login that fails to spawn emits its
 * `done` event immediately — lost in that gap, it would leave `running` true
 * with no way back. `onEstablished` also fires on every web-socket reconnect,
 * so the start is latched to the first one.
 */
export function startAgentLogin(checkId: string): Promise<void> {
  // One record, one login: taking it over would strand the running fix's
  // awaiter and hide the record it is still writing to. The backend refuses a
  // second login for the same check anyway.
  if (agentLogin.running) {
    return Promise.reject(new Error(`A login is already running for ${agentLogin.checkId}`));
  }
  stopWatching();
  agentLogin.checkId = checkId;
  agentLogin.running = true;
  agentLogin.url = null;
  agentLogin.output = [];
  agentLogin.error = null;
  agentLogin.code = '';
  agentLogin.sending = false;

  return new Promise<void>((resolve, reject) => {
    let started = false;
    let settled = false;
    const settle = (error: string | null) => {
      if (settled) return;
      settled = true;
      agentLogin.running = false;
      agentLogin.sending = false;
      stopWatching();
      if (error === null) {
        resolve();
        return;
      }
      agentLogin.error = error;
      reject(new Error(error));
    };

    unlisten = listenToEvent<DoctorLoginOutput>(
      'doctor-login-output',
      (output) => {
        if (output.checkId !== checkId) return;
        if (output.line !== null) recordLine(output.line);
        if (output.done) settle(output.error ?? null);
      },
      {
        onEstablished: () => {
          if (started) return;
          started = true;
          void startDoctorLogin(checkId).catch((e) => settle(errorText(e)));
        },
      }
    );
  });
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
