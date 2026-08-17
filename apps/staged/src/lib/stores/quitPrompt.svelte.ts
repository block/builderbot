/**
 * State behind the quit confirmation dialog.
 *
 * The backend raises `app:quit-requested` when the user quits with sessions
 * still active (see `app_lifecycle.rs`); quitListener.ts feeds that payload in
 * here and QuitConfirmDialog renders it. Answering is a round trip back to the
 * backend: confirming hands off to the shutdown sequence, which stops the
 * sessions and then exits the process — so the dialog stays up, in its
 * `stopping` state, until the app goes away underneath it.
 */

import * as commands from '../api/commands';
import type { QuitRequestedPayload } from '../types';

class QuitPromptStore {
  private _payload = $state<QuitRequestedPayload | null>(null);
  /** The quit was confirmed and the backend is stopping sessions. */
  private _stopping = $state(false);

  get payload(): QuitRequestedPayload | null {
    return this._payload;
  }

  get open(): boolean {
    return this._payload !== null;
  }

  get stopping(): boolean {
    return this._stopping;
  }

  /** A quit is waiting on the user's answer. */
  requested(payload: QuitRequestedPayload): void {
    this._payload = payload;
    this._stopping = false;
  }

  /** Quit and stop the listed sessions. */
  async confirm(): Promise<void> {
    if (this._stopping) return;
    this._stopping = true;
    try {
      await commands.confirmQuit();
    } catch (e) {
      // The quit never started, so drop the dialog rather than leaving it stuck
      // on "Stopping sessions…" for an app that isn't going anywhere.
      console.error('Failed to confirm quit:', e);
      this._payload = null;
      this._stopping = false;
    }
  }

  /** Keep running. Also the Esc / click-outside path. */
  cancel(): void {
    if (this._stopping) return;
    this._payload = null;
    void commands.cancelQuit().catch((e) => console.error('Failed to cancel quit:', e));
  }
}

export const quitPrompt = new QuitPromptStore();
