import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
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
 * Resolves true when the session resumed, and false — without a toast — when
 * a resume of the same session is already in flight.
 */
export async function resumeSessionFromRow(
  sessionId: string,
  branchId: string | null
): Promise<boolean> {
  if (resumesInFlight.has(sessionId)) return false;
  resumesInFlight.add(sessionId);
  try {
    await commands.resumeSession(sessionId, RESUME_ROW_PROMPT, undefined, branchId);
    return true;
  } catch (e) {
    console.error('Failed to resume session:', e);
    toast.error('Resume failed', {
      description:
        e instanceof Error ? e.message : 'Could not resume the session. Please try again.',
    });
    return false;
  } finally {
    resumesInFlight.delete(sessionId);
  }
}
