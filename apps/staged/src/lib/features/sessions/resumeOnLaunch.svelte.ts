/**
 * The launch-time "Resume interrupted sessions?" offer.
 *
 * The backend computes the set once per process (see `launch_resume`); the
 * desktop main window asks for it after the store status is known and shows
 * the dialog when there is something to offer. The dialog lists one row per
 * project; every row that can be resumed starts checked. "Resume" sends back
 * the resumable sessions of the checked projects; "Dismiss" sends an empty
 * list. Both record the backend's internal handled marker, so no row (checked
 * or not) is offered again on the next launch; the rows left alone keep their
 * per-session Resume buttons.
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type { LaunchRecovery } from '../../types';
import {
  defaultSelection,
  groupByProject,
  interruptedCount,
  rowSelectable,
  selectedSessionIds,
} from './resumeOnLaunchGroups';

class ResumeOnLaunchController {
  private _pending = $state<LaunchRecovery | null>(null);
  private _busy = $state(false);
  /** Keys of the checked project rows; replaced, never mutated, so it stays reactive. */
  private _selected = $state<string[]>([]);

  /** The offer awaiting an answer, or null when the dialog is closed. */
  get pending(): LaunchRecovery | null {
    return this._pending;
  }

  /** A "Resume" or "Dismiss" is in flight. */
  get busy(): boolean {
    return this._busy;
  }

  /** Keys of the checked project rows (see `groupByProject`). */
  get selected(): string[] {
    return this._selected;
  }

  /** The session ids "Resume" would send now. */
  get selectedSessionIds(): string[] {
    return this._pending ? selectedSessionIds(this._pending, this._selected) : [];
  }

  /** "Resume" can be pressed: something is checked and no answer is in flight. */
  get canResume(): boolean {
    return !this._busy && this.selectedSessionIds.length > 0;
  }

  isSelected(key: string): boolean {
    return this._selected.includes(key);
  }

  /** Check or uncheck one project row. Rows with nothing resumable stay unchecked. */
  setSelected(key: string, selected: boolean): void {
    if (!this._pending || this._busy) return;
    const without = this._selected.filter((k) => k !== key);
    if (!selected) {
      this._selected = without;
      return;
    }
    const row = groupByProject(this._pending).find((r) => r.key === key);
    if (row && rowSelectable(row)) this._selected = [...without, key];
  }

  /** Check every row that can be resumed, or uncheck them all. */
  setAllSelected(selected: boolean): void {
    if (!this._pending || this._busy) return;
    this._selected = selected ? defaultSelection(this._pending) : [];
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
    this._selected = defaultSelection(recovery);
    this._pending = recovery;
    return true;
  }

  /**
   * Resume the checked projects' sessions, repo sessions first. The rest are
   * left as they are and, like a dismissal, not offered again.
   */
  async resume(): Promise<void> {
    if (!this.canResume) return;
    await this.answer((recovery) => selectedSessionIds(recovery, this._selected));
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
      this._selected = [];
    }
  }
}

export const resumeOnLaunch = new ResumeOnLaunchController();
