import type { NotesChangedEvent } from '../../types';

/** The identity an open note viewer filters `notes-changed` events against. */
export interface NoteRefreshTarget {
  noteKind: 'branch' | 'project';
  branchId?: string | null;
  projectId?: string | null;
}

/**
 * Could a `notes-changed` event concern the note a viewer is showing?
 *
 * The store scopes a branch-note write to its branch (`{ branchId, projectId: null }`)
 * and a project-note write to its project (`{ branchId: null, projectId }`), so a
 * viewer matches on the id for its own kind. Two cases widen to "refetch":
 *
 *   - the feed's lag recovery, which nulls every id because it can no longer
 *     say what changed;
 *   - a viewer that doesn't know its own scoping id — reference-history entries
 *     can open a note without a branch — where one note fetch is far cheaper
 *     than leaving stale content on screen.
 */
export function notesChangeAffectsNote(
  payload: NotesChangedEvent,
  target: NoteRefreshTarget
): boolean {
  if (payload.branchId === null && payload.projectId === null) return true;
  const isProject = target.noteKind === 'project';
  const scopeId = isProject ? target.projectId : target.branchId;
  if (!scopeId) return true;
  return (isProject ? payload.projectId : payload.branchId) === scopeId;
}

/** What names the note a viewer is showing, as the viewer's host passes it. */
export interface NoteIdentitySource {
  noteKind: 'branch' | 'project';
  sessionId?: string | null;
  noteId?: string | null;
}

/**
 * A stable key for the note a viewer is showing; null when nothing names one.
 *
 * It follows the viewer's own lookup: the session when there is one — a
 * session owns a single note, and a note opened before its runner wrote it
 * has no id yet — and the note id otherwise. So the key holds still when a
 * host learns the id of a note it opened by session, and changes exactly when
 * the viewer's fetch would return a different row.
 */
export function noteIdentityKey({
  noteKind,
  sessionId,
  noteId,
}: NoteIdentitySource): string | null {
  if (sessionId) return `${noteKind}:session:${sessionId}`;
  if (noteId) return `${noteKind}:note:${noteId}`;
  return null;
}

/** A row a viewer fetched while open, tagged with the note it was fetched for. */
export interface FetchedNote {
  /** The {@link noteIdentityKey} the fetch was made under. */
  identity: string;
  id: string;
  title: string;
  content: string;
  updatedAt: number;
}

/**
 * The fetched row a viewer should show in place of its snapshot props, if any.
 *
 * A viewer's props are a snapshot taken when the note was clicked, and hosts
 * hand that same snapshot over again whenever they rebuild their state (every
 * chat toggle does). So props arriving is no sign that they are newer than
 * what the viewer fetched. The fetched row stands until the props name
 * another note, or carry a copy of this one written after it.
 */
export function liveNoteFor(
  fetched: FetchedNote | null,
  identity: string | null,
  snapshotUpdatedAt: number | null | undefined
): FetchedNote | null {
  if (!fetched || fetched.identity !== identity) return null;
  if (typeof snapshotUpdatedAt === 'number' && snapshotUpdatedAt > fetched.updatedAt) return null;
  return fetched;
}
