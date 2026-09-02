import type { CommitTimelineItem } from '../../types';
import { isSessionActive } from '../../shared/sessionStatus';

/**
 * Whether a rebase pipeline session is already queued or running on the branch.
 *
 * Both paths leave a pending commit row in the timeline that carries the
 * pipeline kind, so this reads that structured field rather than the row's
 * subject — the subject is a display label an agent-pushed ACP title can
 * replace once a conflicted rebase hands off to an agent.
 *
 * The header Rebase button hides while this is true: clicking it again would
 * only re-request work the backend dedupes anyway, and the timeline already
 * shows the rebase row. Rendering nothing beats a disabled control here — the
 * button comes back on its own when the rebase finishes and the parent is
 * still ahead.
 */
export function rebaseInFlight(
  commits: Pick<CommitTimelineItem, 'pipelineKind' | 'sessionStatus'>[] | undefined
): boolean {
  return !!commits?.some((c) => c.pipelineKind === 'rebase' && isSessionActive(c.sessionStatus));
}
