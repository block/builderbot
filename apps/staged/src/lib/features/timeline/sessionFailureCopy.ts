import {
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_WORKSTATION_GONE,
} from '../../shared/disconnectedSession';

export type ArtifactKind = 'commit' | 'note' | 'review';

const artifactNoun: Record<ArtifactKind, string> = {
  commit: 'commit',
  note: 'note',
  review: 'comments',
};

/**
 * Reconcile messages on a Blox session Staged ended because it couldn't
 * follow the turn, not because anyone stopped it: the row's `interrupted`
 * reason would otherwise read as a user Stop.
 */
const RECONCILE_INTERRUPTIONS: ReadonlySet<string> = new Set([
  RECONCILE_TURN_UNDETERMINED,
  RECONCILE_NO_WORKSPACE,
  RECONCILE_PROMPT_NOT_RECEIVED,
]);

/**
 * The subtitle of a timeline row whose session ended without producing its
 * artifact. `errorMessage` is the session's: reconcile's messages say more
 * than its completion reason does. `rateLimitResetsAt` dates a rate-limit
 * pause's subtitle.
 */
export function failedArtifactSubtitle(
  completionReason: string | null | undefined,
  kind: ArtifactKind,
  errorMessage?: string | null,
  rateLimitResetsAt?: string | null
): string {
  const noun = artifactNoun[kind];
  if (errorMessage === RECONCILE_WORKSTATION_GONE) {
    return `Workstation deleted — no ${noun} created`;
  }
  if (errorMessage && RECONCILE_INTERRUPTIONS.has(errorMessage)) {
    return `Session interrupted — no ${noun} created`;
  }
  switch (completionReason) {
    case 'crashed':
      return `Session crashed — no ${noun} created`;
    case 'remote_agent_lost':
      return `Workspace agent lost — no ${noun} created`;
    case 'rate_limited':
      return rateLimitPausedSubtitle(rateLimitResetsAt) ?? `Paused by API limit — no ${noun} yet`;
    case 'app_quit':
      return `Session interrupted — no ${noun} created`;
    case 'project_session_interrupted':
      return `Session stopped by project session — no ${noun} created`;
    case 'interrupted':
      return `Session stopped — no ${noun} created`;
    case 'held_until_cap':
    case 'hold_stopped':
      return `Session stopped waiting on background work — no ${noun} created`;
    default:
      return `Session finished — no ${noun} created`;
  }
}

export function rateLimitPausedSubtitle(rateLimitResetsAt?: string | null): string | null {
  const resetTime = formatRateLimitResetTime(rateLimitResetsAt);
  if (resetTime) return `Paused by API limit — resets around ${resetTime}`;
  return null;
}

export function formatRateLimitResetTime(rateLimitResetsAt?: string | null): string | null {
  if (!rateLimitResetsAt) return null;
  const date = new Date(rateLimitResetsAt);
  if (Number.isNaN(date.getTime())) return null;

  return date.toLocaleTimeString([], {
    hour: 'numeric',
    minute: '2-digit',
  });
}
