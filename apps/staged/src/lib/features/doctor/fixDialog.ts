/**
 * fixDialog.ts — the one decision the Doctor panel's fix dialog makes on its way
 * out, kept pure so it can be tested without rendering the row: whether leaving
 * the dialog also ends the login it shows.
 */
import type { AgentLoginOrigin } from './agentLogin.svelte';

/** What the dialog knows about the login the shared record shows for its check. */
export interface FixDialogLogin {
  /** The shared record shows a login running for this dialog's check. */
  running: boolean;
  /**
   * This dialog asked for that login — its Run was confirmed — as opposed to
   * having attached, on open, to one already running: started from the session
   * pane, from another client, or before this view reloaded. Only what the
   * dialog knows on its own; whether the request in fact began a run is
   * `origin`, and it takes both to call the login the dialog's.
   */
  requestedHere: boolean;
  /**
   * The record's `origin`: how this client came to follow the run. `started`
   * when the request began it, `attached` when the backend answered "already
   * running" or a probe found it, null while a start's answer is still in flight.
   */
  origin: AgentLoginOrigin | null;
}

/**
 * Whether closing the fix dialog — its Cancel, Escape, a click outside — should
 * cancel the login it shows, rather than merely stop watching it.
 *
 * A login the dialog started has no other watcher. Left running, it would hold
 * the check's login slot until doctor's fix timeout: the CLI ignores a closed
 * stdin once it is waiting on its browser callback, so only a kill ends it. A
 * login the dialog attached to is someone else's to watch — the session pane
 * that started it is still showing its URL and code box — and opening this
 * dialog for a look must not kill it on the way out. That includes a login the
 * dialog *asked* for but the backend answered "already running": the probe on
 * open found nothing, someone else started one before Run was clicked, and the
 * click re-attached to their login. The dialog cannot tell that from its own
 * request; the record's `origin` can, so it is read here rather than assumed.
 *
 * A request the backend has yet to answer is cancelled. It is this dialog's,
 * and the store's cancel waits for the answer to name the run before asking.
 * Should that answer turn out to be "already running", the kill lands on a login
 * the user asked to start a moment ago and then left — the ambiguous shape:
 * after a reload whose probe failed it is their own lost login, and ending it is
 * right; otherwise it is someone else's. Resolved for cancelling, because the
 * other reading leaves a slot the user has walked away from held until doctor's
 * fix timeout, and the window is the backend's answer, not a human's.
 *
 * Nothing running means nothing to decide.
 */
export function closingFixDialogCancelsLogin(login: FixDialogLogin): boolean {
  return login.running && login.requestedHere && login.origin !== 'attached';
}
