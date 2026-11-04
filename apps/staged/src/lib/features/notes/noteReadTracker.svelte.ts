/**
 * The note viewer's read state. Each viewer instance owns one tracker.
 *
 * The viewer calls `show` whenever a note comes on screen — when it opens, and
 * when reference navigation swaps in another note — and that marks the note
 * read. Refreshes while the note stays on screen only update what the toggle
 * shows, so a manual "Mark as unread" sticks until the note is shown again.
 * The one exception is a note whose content lands while it is on screen (its
 * session finishes in the open viewer): the user watched it land, so it is
 * marked read then. There was nothing to mark unread before that.
 */

import {
  setNoteReadState,
  writeNoteRead,
  type NoteKind,
  type NoteReadRow,
} from './noteReadActions';
import { isNoteUnread } from './noteTimestamps';

interface ShownNote {
  kind: NoteKind;
  noteId: string;
  completedAt: number | null;
  readAt: number | null;
  /** A child note aggregated under a project note, which no timeline lists. */
  child: boolean;
}

/** The read fields of any fetched note row. */
export interface NoteReadSnapshot {
  id: string;
  completedAt: number | null;
  readAt: number | null;
}

function shownNote(kind: NoteKind, row: NoteReadRow): ShownNote {
  return {
    kind,
    noteId: row.id,
    completedAt: row.completedAt,
    readAt: row.readAt,
    child: 'parentProjectNoteId' in row && row.parentProjectNoteId !== null,
  };
}

export class NoteReadTracker {
  private shown = $state<ShownNote | null>(null);
  private toggleInFlight = $state(false);
  /** `kind:id` of the note last shown since the viewer opened. */
  private shownKey: string | null = null;
  /** Bumped on every show and reset, so a write that resolves late is dropped. */
  private generation = 0;

  /**
   * Whether the viewer offers the toggle: the note has landed, and it is one
   * a timeline lists (child notes only appear under their project note).
   */
  get canToggle(): boolean {
    const shown = this.shown;
    return !!shown && shown.completedAt !== null && !shown.child;
  }

  get unread(): boolean {
    return !!this.shown && isNoteUnread(this.shown);
  }

  get toggling(): boolean {
    return this.toggleInFlight;
  }

  /** The viewer is showing this note; marks it read unless it already was this open. */
  show(kind: NoteKind, noteId: string): void {
    const key = `${kind}:${noteId}`;
    if (key === this.shownKey) return;
    this.shownKey = key;
    const generation = ++this.generation;
    this.shown = null;
    this.toggleInFlight = false;
    writeNoteRead(kind, noteId, true).then(
      (row) => {
        if (generation === this.generation) this.shown = shownNote(kind, row);
      },
      (error) => {
        console.warn('Failed to mark note read', error);
      }
    );
  }

  /** The viewer closed, so the next `show` marks read again. */
  reset(): void {
    this.shownKey = null;
    this.generation++;
    this.shown = null;
    this.toggleInFlight = false;
  }

  /** A fresh row for the note on screen, e.g. after a write from another window. */
  observe(note: NoteReadSnapshot): void {
    const shown = this.shown;
    if (!shown || shown.noteId !== note.id) return;
    const landed = shown.completedAt === null && note.completedAt !== null;
    this.shown = { ...shown, completedAt: note.completedAt, readAt: note.readAt };
    if (!landed) return;
    const generation = this.generation;
    writeNoteRead(shown.kind, shown.noteId, true).then(
      (row) => this.apply(generation, row),
      (error) => {
        console.warn('Failed to mark note read', error);
      }
    );
  }

  /** Flip the note on screen between read and unread at the user's request. */
  async toggle(projectId: string | null | undefined): Promise<void> {
    const shown = this.shown;
    if (!shown || !this.canToggle || this.toggleInFlight) return;
    const generation = this.generation;
    this.toggleInFlight = true;
    try {
      const row = await setNoteReadState(
        { kind: shown.kind, noteId: shown.noteId, projectId },
        isNoteUnread(shown)
      );
      if (row) this.apply(generation, row);
    } finally {
      if (generation === this.generation) this.toggleInFlight = false;
    }
  }

  private apply(generation: number, row: NoteReadRow): void {
    const shown = this.shown;
    if (generation !== this.generation || shown?.noteId !== row.id) return;
    this.shown = { ...shown, completedAt: row.completedAt, readAt: row.readAt };
  }
}
