/**
 * Badge assembly for the repo sessions a running project session spawned.
 *
 * A running project session normally shows its newest message as a
 * subtitle. When it has repo sessions in flight, those badges say more than
 * the hint does, so they take the subtitle's place until the children finish.
 */

import type { ChildSessionInfo } from '../../types';
import { sessionRowTitle, truncateTitle, type SessionRowKind } from './sessionRowTitle';

/**
 * Longest label a badge shows before clipping. The project column is narrow,
 * and the full title stays reachable through the badge's tooltip.
 */
export const CHILD_BADGE_TITLE_MAX_CHARS = 36;

export interface ChildSessionBadgeProps {
  /** Child session id — unique per badge, so two children in one repo keep two badges. */
  key: string;
  /** Drives the badge's colour, the same per-kind colour the branch timeline uses. */
  sessionType: SessionRowKind;
  /**
   * Queued children wait on the branch queue; running ones get a spinner, and
   * disconnected ones (which may still be running remotely) a cloud-off icon.
   */
  status: 'queued' | 'running' | 'disconnected';
  /** The clipped label. */
  label: string;
  /** The whole title, for the tooltip. */
  title: string;
}

function asRowKind(sessionType: string): SessionRowKind {
  return sessionType === 'commit' || sessionType === 'review' ? sessionType : 'note';
}

/**
 * Project in-flight children onto badge props.
 *
 * Each badge is titled the way the branch timeline would title the same
 * session (`sessionRowTitle`), with `liveTitle` — a session's live ACP title
 * from the session stream — layered on top exactly as BranchTimeline layers
 * it on its rows. It is asked only for the children `runningChildSessionIds`
 * names, which are the ones the stream was asked for.
 */
export function buildChildSessionBadges(
  children: ChildSessionInfo[],
  liveTitle: (sessionId: string) => string | undefined = () => undefined
): ChildSessionBadgeProps[] {
  return children.map((child) => {
    const kind = asRowKind(child.sessionType);
    const title = sessionRowTitle(
      kind,
      child.title,
      hasLiveTitle(child) ? liveTitle(child.sessionId) : undefined
    );
    return {
      key: child.sessionId,
      sessionType: kind,
      status: child.status,
      label: truncateTitle(title, CHILD_BADGE_TITLE_MAX_CHARS),
      title,
    };
  });
}

/** A child whose title can still change. A queued one has no agent yet. */
function hasLiveTitle(child: ChildSessionInfo): boolean {
  return child.status === 'running';
}

/**
 * The children whose titles can still change: these are asked of the session
 * stream at its title level (status and title, no transcript rows) alongside
 * their parents, so a badge picks up an ACP title the way the branch card's
 * row does.
 */
export function runningChildSessionIds(childrenBySession: Record<string, ChildSessionInfo[]>) {
  const ids: string[] = [];
  for (const children of Object.values(childrenBySession)) {
    for (const child of children) {
      if (hasLiveTitle(child)) ids.push(child.sessionId);
    }
  }
  return ids;
}

/**
 * The subtitle a running project-session row should show: the live hint,
 * unless child badges have taken the row's meta line over.
 */
export function runningSecondaryMeta(
  badges: ChildSessionBadgeProps[],
  liveHint: string | undefined
): string | undefined {
  if (badges.length > 0) return undefined;
  return liveHint ?? 'Generating note';
}
