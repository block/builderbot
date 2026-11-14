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
 * `projectStateStore.markUnreadForNote`). The project flagged is the one the
 * note is in, whichever project's view the user marked it from.
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
}

function noteKey(kind: NoteKind, noteId: string): string {
  return `${kind}:${noteId}`;
}

/**
 * "Mark as unread" flags the note's project only after it has found which
 * project that is, and for a branch note no view has loaded that is a backend
 * lookup. Until it answers the flag is not set, so a read mark or a delete
 * landing in that time finds nothing to release, and the flag set afterwards
 * would stay on a note that is read, or gone. Each such flag is held here, by
 * note, as a claim taken when its unread write lands. Any later write or
 * delete of the note takes the claim away, and a flag whose claim is gone is
 * not set.
 */
const unreadFlagClaims = new Map<string, symbol>();

/**
 * Write a note's read state. Every way a note gets read goes through here — the
 * viewer showing it, the editor opening it, "Mark as read" — so marking it read
 * also releases the project unread flag its "Mark as unread" set.
 */
export function writeNoteRead(kind: NoteKind, noteId: string, read: boolean): Promise<NoteReadRow> {
  return landNoteRead(kind, noteId, read, null);
}

/**
 * {@link writeNoteRead}, leaving `claim` as the note's unread flag claim once
 * the write has landed (or none, for null). In the same step as the landing,
 * so no other write's landing can come between the two.
 */
async function landNoteRead(
  kind: NoteKind,
  noteId: string,
  read: boolean,
  claim: symbol | null
): Promise<NoteReadRow> {
  const row = await (kind === 'project'
    ? commands.setProjectNoteRead(noteId, read)
    : commands.setNoteRead(noteId, read));
  const key = noteKey(kind, noteId);
  if (claim) unreadFlagClaims.set(key, claim);
  else unreadFlagClaims.delete(key);
  if (read) {
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
  const key = noteKey(kind, noteId);
  unreadFlagClaims.delete(key);
  projectStateStore.forgetNote(key);
}

/**
 * The project a note is in. A project note's row names it; a branch note's is
 * its branch's, which a view may not have loaded — a `#note:<id>` reference
 * can open a note from any project.
 */
async function owningProjectId(row: NoteReadRow): Promise<string | null> {
  if ('projectId' in row) return row.projectId;
  for (const [projectId, branches] of projectsDataStore.branchesByProject) {
    if (branches.some((branch) => branch.id === row.branchId)) return projectId;
  }
  try {
    return (await commands.getBranch(row.branchId))?.projectId ?? null;
  } catch (error) {
    console.warn('Failed to look up the note’s project', error);
    return null;
  }
}

/**
 * Set a note's read state at the user's request. Resolves to the row as it
 * now reads, or null once a failure has been toasted.
 */
export async function setNoteReadState(
  target: NoteReadTarget,
  read: boolean
): Promise<NoteReadRow | null> {
  const key = noteKey(target.kind, target.noteId);
  const claim = read ? null : Symbol(key);
  let row: NoteReadRow;
  try {
    row = await landNoteRead(target.kind, target.noteId, read, claim);
  } catch (e) {
    toast.error(read ? 'Unable to mark note as read' : 'Unable to mark note as unread', {
      description: e instanceof Error ? e.message : String(e),
    });
    return null;
  }
  if (claim) {
    const projectId = await owningProjectId(row);
    // The note was read, deleted or marked again while its project was being
    // looked up (see unreadFlagClaims): that write decides the flag.
    if (unreadFlagClaims.get(key) !== claim) return row;
    unreadFlagClaims.delete(key);
    if (projectId && !projectsDataStore.isProjectDeleting(projectId)) {
      projectStateStore.markUnreadForNote(projectId, key);
    }
  }
  return row;
}
