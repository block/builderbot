/**
 * What a project-notes row reads: the type, title and subtitle ProjectSection
 * hands TimelineRow for one note. The branch timeline builds its note items
 * the same way; the two differ in that a project stub is untitled, so a failed
 * project row shows the failure copy as its title with no subtitle.
 */

import type { ProjectNote } from '../../types';
import type { TimelineItemType } from '../timeline/TimelineRow.svelte';
import { formatNoteTimestamp } from '../notes/noteTimestamps';
import { runningSecondaryMeta, type ChildSessionBadgeProps } from '../timeline/childSessionBadges';
import { failedArtifactSubtitle } from '../timeline/sessionFailureCopy';
import { noteRowPhase } from '../timeline/sessionRowState';

export type ProjectNoteRow = {
  type: TimelineItemType;
  title: string;
  secondaryMeta: string | undefined;
};

export type ProjectNoteRowSource = Pick<
  ProjectNote,
  | 'sessionId'
  | 'sessionStatus'
  | 'content'
  | 'title'
  | 'completionReason'
  | 'createdAt'
  | 'updatedAt'
  | 'completedAt'
  | 'rateLimitResetsAt'
>;

export type ProjectNoteRowLive = {
  deleting: boolean;
  /** Clock for the done row's relative timestamp. */
  nowMs: number;
  /** The running session's ACP title, once it has one. */
  liveTitle?: string;
  /** The running session's latest agent message. */
  liveHint?: string;
  /** Repo sessions the running session has in flight; they take the subtitle over. */
  childBadges?: ChildSessionBadgeProps[];
};

export function projectNoteRow(
  note: ProjectNoteRowSource,
  live: ProjectNoteRowLive
): ProjectNoteRow {
  let row: ProjectNoteRow;
  switch (noteRowPhase(note)) {
    // Project sessions start running, so a queued status never reaches a
    // project note today; if one did it would read as running, as before.
    case 'queued':
    case 'running':
      row = {
        type: 'generating-note',
        title: live.liveTitle ?? 'Generating note…',
        secondaryMeta: runningSecondaryMeta(live.childBadges ?? [], live.liveHint),
      };
      break;
    case 'failed':
      row = {
        type: 'failed-note',
        title: failedArtifactSubtitle(note.completionReason, 'note', note.rateLimitResetsAt),
        secondaryMeta: undefined,
      };
      break;
    case 'done':
      row = {
        type: 'note',
        title: note.title || 'Untitled note',
        secondaryMeta: formatNoteTimestamp(note, live.nowMs),
      };
      break;
  }
  if (live.deleting) row.secondaryMeta = 'Deleting...';
  return row;
}
