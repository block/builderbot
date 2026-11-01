/**
 * The title a branch timeline row gives an in-flight session, by kind.
 *
 * The backend already resolves each kind's stored title the branch timeline's
 * way (a commit's ACP title else its prompt, a note's title, a review's title
 * else — while running — its ACP title). This is the frontend half of that
 * rule: the live ACP title the hint poller fetches overrides the stored one
 * for commits and reviews, never for notes, and commit and note titles are
 * stripped of XML tags because a session label can be the raw prompt.
 *
 * BranchTimeline's rows and the child-session badges under a project session
 * both go through here so a child is named the same in both places.
 */

import { stripXmlTags } from '../sessions/sessionModalHelpers';

export type SessionRowKind = 'commit' | 'note' | 'review';

export const UNTITLED_REVIEW_TITLE = 'Code Review';

export function sessionRowTitle(
  kind: SessionRowKind,
  storedTitle: string | null | undefined,
  liveTitle: string | undefined
): string {
  switch (kind) {
    case 'commit':
      return stripXmlTags(liveTitle ?? storedTitle ?? '');
    case 'note':
      return stripXmlTags(storedTitle ?? '');
    case 'review':
      return liveTitle ?? (storedTitle || UNTITLED_REVIEW_TITLE);
  }
}

/**
 * Clip a title to `maxChars` characters, ending in an ellipsis. Counts code
 * points, not UTF-16 units, so an emoji is never split. Whitespace runs are
 * collapsed first so a multi-line prompt reads as one line.
 */
export function truncateTitle(title: string, maxChars: number): string {
  const collapsed = title.replace(/\s+/g, ' ').trim();
  const chars = Array.from(collapsed);
  if (chars.length <= maxChars) return collapsed;
  return `${chars
    .slice(0, Math.max(1, maxChars - 1))
    .join('')
    .trimEnd()}…`;
}
