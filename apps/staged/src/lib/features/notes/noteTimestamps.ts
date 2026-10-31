import { formatRelativeTime } from '../../shared/relativeTime';

/** The timestamp columns shared by branch notes and project notes (all in ms). */
export interface NoteTimestamps {
  createdAt: number;
  updatedAt: number;
  completedAt: number | null;
}

/**
 * Has the note's content changed since it first landed?
 *
 * `completedAt` is set once, when content first lands (or the session ends
 * without any), while `updatedAt` moves on every content change. Notes from
 * before `completedAt` existed were backfilled with `completedAt = updatedAt`,
 * so they read as unrevised.
 */
export function isNoteRevised(note: NoteTimestamps): boolean {
  return note.completedAt !== null && note.updatedAt > note.completedAt;
}

/** When the note's content was last written; the key both note lists sort by. */
export function noteLastWrittenAt(note: NoteTimestamps): number {
  return isNoteRevised(note) ? note.updatedAt : (note.completedAt ?? note.createdAt);
}

/** Row label for a note: "Updated 5m ago" once revised, otherwise "5m ago". */
export function formatNoteTimestamp(note: NoteTimestamps, nowMs: number): string {
  const relative = formatRelativeTime(noteLastWrittenAt(note), nowMs);
  return isNoteRevised(note) ? `Updated ${relative}` : relative;
}
