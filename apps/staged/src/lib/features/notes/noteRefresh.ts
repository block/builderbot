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
