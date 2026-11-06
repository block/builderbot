/**
 * Row state shared by the two lists that render session-backed timeline rows:
 * a branch card's timeline (BranchTimeline) and a project's notes list
 * (ProjectSection). Both mount the same TimelineRow; these rules decide what
 * each parent hands it, so a session reads the same in either place.
 *
 * Branch-only gates stay with the branch timeline. A branch has a single
 * worktree, so it withholds Resume while any session on it is running; project
 * sessions run side by side and have no such gate.
 */

import { isSessionInFlight } from '../../shared/sessionStatus';
import { isResumableReason } from '../../types';

/** The prompt a row's Resume button sends to the session it resumes. */
export const RESUME_ROW_PROMPT = 'Continue where you left off.';

/**
 * What a session row shows: waiting in the branch queue, running, finished
 * without writing its artifact, or done.
 */
export type SessionRowPhase = 'queued' | 'running' | 'failed' | 'done';

/**
 * The phase of a note row. A disconnected session is still in flight on its
 * workstation, so it reads as running. A note whose session has finished but
 * whose content is still empty failed: the session ended without writing the
 * note.
 * A note with no session is a note the user wrote, so it is always done.
 */
export function noteRowPhase(note: {
  sessionId?: string | null;
  sessionStatus?: string | null;
  content?: string | null;
}): SessionRowPhase {
  if (note.sessionStatus === 'queued') return 'queued';
  if (isSessionInFlight(note.sessionStatus)) return 'running';
  if (note.sessionId && !note.content?.trim()) return 'failed';
  return 'done';
}

/**
 * Whether a row offers Resume: it has a session, the session ended for a
 * resumable reason, the row isn't being deleted, and the session isn't queued
 * or running already.
 *
 * Completed rows count too. A note written by a turn whose background hold was
 * stopped (`hold_stopped`, `held_until_cap`) is done and still worth nudging.
 */
export function canResumeRow(row: {
  sessionId?: string | null;
  completionReason?: string | null;
  deleting?: boolean;
  phase: SessionRowPhase;
}): boolean {
  if (!row.sessionId || row.deleting) return false;
  if (row.phase === 'queued' || row.phase === 'running') return false;
  return isResumableReason(row.completionReason);
}
