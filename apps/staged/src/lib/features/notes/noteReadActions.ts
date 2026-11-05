/**
 * Marking a note or project note read or unread: the timeline context menus
 * and the note viewer's toggle go through `setNoteReadState`, and the viewer's
 * mark-on-open through `writeNoteRead`.
 *
 * Marking a note unread also marks its project unread, so the project carries
 * the sidebar's unread dot — as when one of its sessions finishes while the
 * user is elsewhere. Reading the note again, however that happens, or deleting
 * it takes the dot away with it, unless another note marked unread still holds
 * it or the project was flagged some other way (see
 * `projectStateStore.markUnreadForNote`).
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import { projectStateStore } from '../../stores/projectState.svelte';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import type { BranchNote, ProjectNote } from '../../types';

export type NoteKind = 'branch' | 'project';

/** The row a read-state write returns, as the note now reads. */
export type NoteReadRow = BranchNote | ProjectNote;

export interface NoteReadTarget {
  kind: NoteKind;
  noteId: string;
  /** The project a branch note is in. A project note's row names its own. */
  projectId?: string | null;
}

function noteKey(kind: NoteKind, noteId: string): string {
  return `${kind}:${noteId}`;
}

/**
 * Write a note's read state. Every way a note gets read goes through here — the
 * viewer showing it, the editor opening it, "Mark as read" — so marking it read
 * also releases the project unread flag its "Mark as unread" set.
 */
export async function writeNoteRead(
  kind: NoteKind,
  noteId: string,
  read: boolean
): Promise<NoteReadRow> {
  const row = await (kind === 'project'
    ? commands.setProjectNoteRead(noteId, read)
    : commands.setNoteRead(noteId, read));
  if (read) {
    const key = noteKey(kind, noteId);
    for (const projectId of projectStateStore.projectsFlaggedByNote(key)) {
      if (!projectsDataStore.isProjectDeleting(projectId)) {
        projectStateStore.releaseNoteUnread(projectId, key);
      }
    }
  }
  return row;
}

/**
 * The note was deleted: release the project flags its "Mark as unread" held,
 * as reading it would have. A project being deleted loses its flag with it,
 * so unlike a read this needn't skip one.
 */
export function forgetDeletedNote(kind: NoteKind, noteId: string): void {
  projectStateStore.forgetNote(noteKey(kind, noteId));
}

/**
 * Set a note's read state at the user's request. Resolves to the row as it
 * now reads, or null once a failure has been toasted.
 */
export async function setNoteReadState(
  target: NoteReadTarget,
  read: boolean
): Promise<NoteReadRow | null> {
  let row: NoteReadRow;
  try {
    row = await writeNoteRead(target.kind, target.noteId, read);
  } catch (e) {
    toast.error(read ? 'Unable to mark note as read' : 'Unable to mark note as unread', {
      description: e instanceof Error ? e.message : String(e),
    });
    return null;
  }
  const projectId = 'projectId' in row ? row.projectId : target.projectId;
  if (!read && projectId && !projectsDataStore.isProjectDeleting(projectId)) {
    projectStateStore.markUnreadForNote(projectId, noteKey(target.kind, target.noteId));
  }
  return row;
}
