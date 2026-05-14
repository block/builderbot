import * as commands from '../commands';
import { sessionRegistry } from '../stores/sessionRegistry.svelte';

/**
 * Cancel a running session, delete the linked item, and clean up the registry.
 * Shared by branch note/review/commit deletion and project note deletion.
 */
export async function deleteSessionLinkedItem(
  deleteItem: () => Promise<void>,
  sessionId?: string
): Promise<void> {
  if (sessionId) {
    try {
      await commands.cancelSession(sessionId);
    } catch {
      // Session may already be finished
    }
  }
  await deleteItem();
  if (sessionId) {
    sessionRegistry.cleanupSession(sessionId);
  }
}
