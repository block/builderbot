import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import { QUEUED_FOR_TRANSFER_LABEL } from '../projects/projectTransfer';
import { RESUME_ROW_PROMPT } from './sessionRowState';

/**
 * Sessions whose row Resume is in flight, across every list that offers one.
 * A branch card's button stays up until its timeline reloads, so a quick
 * second click would otherwise fail the backend's running claim and toast a
 * spurious "Resume failed".
 */
const resumesInFlight = new Set<string>();

/**
 * Resume a session from its timeline row's Resume button.
 *
 * `branchId` is the branch the row sits on, or null for a project note's
 * session. A failure is reported with a toast, so callers only need the
 * result to decide whether to refresh their rows.
 *
 * `afterResume` is the caller's refresh — the read that takes its Resume
 * button down. It runs only after the session resumed, and the session stays
 * in flight until it settles, so a second click during the refresh is dropped
 * instead of reaching the backend and toasting "Resume failed". A refresh that
 * rejects is logged; the session did resume, so there is no toast and the
 * result is still true.
 *
 * Resolves true when the session resumed, and false — without a toast — when
 * a resume of the same session is already in flight.
 */
export async function resumeSessionFromRow(
  sessionId: string,
  branchId: string | null,
  afterResume?: () => unknown
): Promise<boolean> {
  if (resumesInFlight.has(sessionId)) return false;
  resumesInFlight.add(sessionId);
  try {
    try {
      const outcome = await commands.resumeSession(
        sessionId,
        RESUME_ROW_PROMPT,
        undefined,
        branchId
      );
      // The project is being transferred: the backend holds the resume as a
      // queued follow-up and sends it once the project has moved.
      if (outcome.outcome === 'queuedForTransfer') toast.info(QUEUED_FOR_TRANSFER_LABEL);
    } catch (e) {
      console.error('Failed to resume session:', e);
      toast.error('Resume failed', {
        description:
          e instanceof Error ? e.message : 'Could not resume the session. Please try again.',
      });
      return false;
    }
    try {
      await afterResume?.();
    } catch (e) {
      console.error('Failed to refresh after resuming session:', e);
    }
    return true;
  } finally {
    resumesInFlight.delete(sessionId);
  }
}
