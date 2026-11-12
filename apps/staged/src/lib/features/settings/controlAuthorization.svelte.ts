/**
 * The "Allow <client> to control Staged?" consent prompt behind
 * `stagedctl connect`.
 *
 * The backend (`control_server/enrol.rs`) queues each request, emits
 * `control-authorization-requested`, and waits for `answer_control_authorization`.
 * Requests are offered one at a time, oldest first. Approve stores a thirty-day
 * grant and hands the waiting CLI its credential; Deny tells it
 * `authorization_denied`; closing the dialog any other way is Deny, so a
 * request never stays silently pending behind a dismissed prompt. A request
 * the user leaves alone lapses on its own at `expiresAt` and is dropped here
 * at the same moment.
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import type { ControlAuthorizationRequest } from '../../types';

class ControlAuthorizationController {
  private _queue = $state<ControlAuthorizationRequest[]>([]);
  private _busy = $state(false);
  private readonly expiryTimers = new Map<string, ReturnType<typeof setTimeout>>();

  /** The request on screen, or null when the dialog is closed. */
  get pending(): ControlAuthorizationRequest | null {
    return this._queue[0] ?? null;
  }

  /** Requests waiting behind the one on screen. */
  get queuedCount(): number {
    return Math.max(0, this._queue.length - 1);
  }

  /** An Approve or Deny is in flight. */
  get busy(): boolean {
    return this._busy;
  }

  /**
   * Queue a request from the event or the startup snapshot. Duplicates (the
   * snapshot overlapping the event) and already-lapsed requests are ignored.
   */
  offer(request: ControlAuthorizationRequest, nowMs = Date.now()): void {
    if (request.expiresAt <= nowMs) return;
    if (this._queue.some((r) => r.requestId === request.requestId)) return;
    this._queue.push(request);
    this.expiryTimers.set(
      request.requestId,
      setTimeout(() => this.drop(request.requestId), request.expiresAt - nowMs)
    );
  }

  /**
   * Take the backend's snapshot of unanswered requests; called once the
   * event listener is live so nothing arriving in between is lost. Swallows
   * failures: a launch must never be blocked by this prompt.
   */
  async loadPending(): Promise<void> {
    let pending: ControlAuthorizationRequest[];
    try {
      pending = await commands.listPendingControlAuthorizations();
    } catch (e) {
      console.error('Failed to load pending stagedctl connection requests:', e);
      return;
    }
    for (const request of pending) this.offer(request);
  }

  async approve(): Promise<void> {
    await this.answer(true);
  }

  async deny(): Promise<void> {
    await this.answer(false);
  }

  private async answer(approve: boolean): Promise<void> {
    const request = this.pending;
    if (!request || this._busy) return;
    this._busy = true;
    try {
      await commands.answerControlAuthorization(request.requestId, approve);
    } catch (e) {
      console.error('Failed to answer a stagedctl connection request:', e);
      toast.error(
        `Could not ${approve ? 'approve' : 'deny'} ${request.client}: ${e instanceof Error ? e.message : e}`
      );
    } finally {
      this._busy = false;
      this.drop(request.requestId);
    }
  }

  private drop(requestId: string): void {
    const timer = this.expiryTimers.get(requestId);
    if (timer !== undefined) clearTimeout(timer);
    this.expiryTimers.delete(requestId);
    const index = this._queue.findIndex((r) => r.requestId === requestId);
    if (index >= 0) this._queue.splice(index, 1);
  }
}

export const controlAuthorization = new ControlAuthorizationController();

/** Wording for what a request (or grant) would reach. */
export function scopeLabel(projectId: string | null, projectName: string | null): string {
  if (projectId === null) return 'every project';
  return projectName ? `the project “${projectName}”` : `one project (${projectId})`;
}

/** Wording for a request's or grant's access level. */
export function accessLabel(access: 'read' | 'write'): string {
  return access === 'read' ? 'read-only' : 'read and write';
}
