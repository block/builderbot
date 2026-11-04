/**
 * Marking a note or project note read or unread: the timeline context menus
 * and the note viewer's toggle go through `setNoteReadState`, and the viewer's
 * mark-on-open through `writeNoteRead`.
 *
 * Marking a note unread also marks its project unread, so the project carries
 * the sidebar's unread dot until it is next opened — as when one of its
 * sessions finishes while the user is elsewhere. Marking one read in the
 * project on screen clears that dot again, as viewing the project would.
 */

import { toast } from 'svelte-sonner';
import * as commands from '../../api/commands';
import { projectStateStore } from '../../stores/projectState.svelte';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { navigation } from '../layout/navigation.svelte';
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

/** Write a note's read state, with no side effects beyond the store's. */
export function writeNoteRead(kind: NoteKind, noteId: string, read: boolean): Promise<NoteReadRow> {
  return kind === 'project'
    ? commands.setProjectNoteRead(noteId, read)
    : commands.setNoteRead(noteId, read);
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
  if (projectId && !projectsDataStore.isProjectDeleting(projectId)) {
    if (!read) {
      projectStateStore.markAsUnread(projectId);
    } else if (
      navigation.selectedProjectId === projectId &&
      projectStateStore.isUnread(projectId)
    ) {
      projectStateStore.markAsRead(projectId);
    }
  }
  return row;
}
