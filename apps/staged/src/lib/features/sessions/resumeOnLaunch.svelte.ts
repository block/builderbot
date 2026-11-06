/**
 * The launch-time "Resume interrupted sessions?" offer.
 *
 * The backend computes the set once per process (see `launch_resume`); the
 * desktop main window asks for it after the store status is known and shows
 * the dialog when there is something to offer. "Resume all" sends back every
 * session that can be restarted; "Dismiss" sends an empty list. Both record
 * the backend's internal handled marker, so the same rows are not offered on
 * the next launch; dismissed rows keep their per-session Resume buttons.
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type { LaunchRecovery } from '../../types';
import { interruptedCount, resumableSessionIds } from './resumeOnLaunchGroups';

class ResumeOnLaunchController {
  private _pending = $state<LaunchRecovery | null>(null);
  private _busy = $state(false);

  /** The offer awaiting an answer, or null when the dialog is closed. */
  get pending(): LaunchRecovery | null {
    return this._pending;
  }

  /** A "Resume all" or "Dismiss" is in flight. */
  get busy(): boolean {
    return this._busy;
  }

  /**
   * Take the launch offer. Resolves to whether the dialog opened. Swallows
   * failures: a launch must never be blocked by this prompt.
   */
  async load(): Promise<boolean> {
    let recovery: LaunchRecovery | null;
    try {
      recovery = await commands.getInterruptedSessions();
    } catch (e) {
      console.error('Failed to look for interrupted sessions:', e);
      return false;
    }
    if (!recovery || interruptedCount(recovery) === 0) return false;
    this._pending = recovery;
    return true;
  }

  /** Restart every session that can be, repo sessions first. */
  async resumeAll(): Promise<void> {
    await this.answer((recovery) => resumableSessionIds(recovery));
  }

  /** Leave every row as it is; just record the offer as handled. */
  async dismiss(): Promise<void> {
    await this.answer(() => []);
  }

  private async answer(sessionIds: (recovery: LaunchRecovery) => string[]): Promise<void> {
    const recovery = this._pending;
    if (!recovery || this._busy) return;
    this._busy = true;
    try {
      await commands.resumeInterruptedSessions(sessionIds(recovery));
    } catch (e) {
      console.error('Failed to answer the interrupted-sessions prompt:', e);
      toast.error(`Could not resume interrupted sessions: ${e instanceof Error ? e.message : e}`);
    } finally {
      this._busy = false;
      this._pending = null;
    }
  }
}

export const resumeOnLaunch = new ResumeOnLaunchController();
