import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import { RESUME_ROW_PROMPT } from './sessionRowState';

/**
 * Resume a session from its timeline row's Resume button.
 *
 * `branchId` is the branch the row sits on, or null for a project note's
 * session. A failure is reported with a toast, so callers only need the
 * result to decide whether to refresh their rows.
 *
 * Resolves true when the session resumed.
 */
export async function resumeSessionFromRow(
  sessionId: string,
  branchId: string | null
): Promise<boolean> {
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
  }
}
