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
 * The header Rebase button hides while this is true, and the `…` menu's Rebase
 * item disables. The backend only dedupes the *queued* case
 * (`queue_commit_pipeline_locked` matches against `find_queued_pipeline`, which
 * reads queued sessions only), so against a rebase that is already **running** a
 * second request inserts a fresh queued rebase that runs once the first lands.
 * This guard is what stops that redundant second rebase, not a cosmetic tidy-up.
 *
 * The header renders nothing rather than a disabled control — it is a transient
 * state with the rebase row already visible in the timeline, and the button
 * comes back on its own if the rebase finishes with the parent still ahead.
 */
export function rebaseInFlight(
  commits: Pick<CommitTimelineItem, 'pipelineKind' | 'sessionStatus'>[] | undefined
): boolean {
  return !!commits?.some((c) => c.pipelineKind === 'rebase' && isSessionActive(c.sessionStatus));
}
