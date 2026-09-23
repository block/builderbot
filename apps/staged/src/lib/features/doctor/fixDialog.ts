/**
 * fixDialog.ts — the one decision the Doctor panel's fix dialog makes on its way
 * out, kept pure so it can be tested without rendering the row: whether leaving
 * the dialog also ends the login it shows.
 */

/** What the dialog knows about the login the shared record shows for its check. */
export interface FixDialogLogin {
  /** The shared record shows a login running for this dialog's check. */
  running: boolean;
  /**
   * This dialog started that login — its Run was confirmed — as opposed to
   * having attached, on open, to one already running: started from the session
   * pane, from another client, or before this view reloaded.
   */
  startedHere: boolean;
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
 * dialog for a look must not kill it on the way out. Nothing running means
 * nothing to decide.
 */
export function closingFixDialogCancelsLogin(login: FixDialogLogin): boolean {
  return login.running && login.startedHere;
}
