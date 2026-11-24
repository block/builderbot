/**
 * The note viewer's diff against the note's previous version: the version
 * that was fetched, the diff computed from it, and which change the reader
 * is pointing at. Each viewer instance owns one.
 *
 * The viewer calls `load` for the note on screen, when it opens and
 * whenever the note is fetched again. The previous version is the newest
 * one the store kept (see `getNotePreviousVersion`); a note that has not
 * been revised has none, and then there is no diff and nothing is shown.
 *
 * The diff is computed from the two versions' blocks (noteDiff.ts): the
 * current ones as the change bar reads them from the rendered note, and the
 * previous ones from the previous version's rendered HTML, parsed off
 * screen. `compare` takes both together, so a render that changes both is
 * one comparison. A comparison of the same blocks as the last keeps the
 * diff it has: its hunks are what the reader points at, and
 * `computeLineDiff` writes to the console on every call.
 *
 * A change is pointed at by hovering its marker, or selected by pressing
 * it. A selected change stays lit, and with room for two panes opens the
 * previous version beside the note (`open`). While both are set the hovered
 * one shows: it is where the pointer is.
 */

import { getNotePreviousVersion } from '../../api/commands';
import type { NoteVersion } from '../../types';
import {
  computeNoteDiff,
  type NoteDiff,
  type NoteDiffHunk,
  type NoteDiffSide,
  type NoteDiffUnit,
} from './noteDiff';
import type { NoteDiffUnitBox } from './noteDiffScroll';
import { extractNoteDiffUnits } from './noteDiffUnits';

/** The note a diff is for: its id, and the text its previous version is previous to. */
export interface NoteDiffSubject {
  id: string;
  title: string;
  content: string;
}

const NO_HUNKS: readonly NoteDiffHunk[] = [];
const NO_BOXES: readonly NoteDiffUnitBox[] = [];

/** What the reader does to a scrolling pane that means they have taken it over. */
const READER_SCROLL_EVENTS = ['wheel', 'touchmove', 'pointerdown', 'keydown'] as const;

function plainUnit({ kind, text, signature }: NoteDiffUnit): NoteDiffUnit {
  return { kind, text, signature };
}

/** Whether two readings hold the same blocks, in the same order. */
export function sameNoteDiffUnits(
  a: readonly NoteDiffUnit[] | null,
  b: readonly NoteDiffUnit[] | null
): boolean {
  if (!a || !b || a.length !== b.length) return false;
  return a.every(
    (unit, i) =>
      unit.kind === b[i].kind && unit.text === b[i].text && unit.signature === b[i].signature
  );
}

function sameBoxes(a: readonly NoteDiffUnitBox[], b: readonly NoteDiffUnitBox[]): boolean {
  return (
    a.length === b.length && a.every((box, i) => box.top === b[i].top && box.height === b[i].height)
  );
}

/**
 * The blocks of a version from its rendered HTML, without showing it. The
 * HTML is parsed into a document of its own, which loads no image and runs
 * nothing, inside a `div` as the viewer's `{@html}` parses it.
 */
export function parseNoteDiffUnits(html: string): NoteDiffUnit[] {
  const parsed = new DOMParser().parseFromString(`<div>${html}</div>`, 'text/html');
  const root = parsed.body.firstElementChild;
  return root ? extractNoteDiffUnits(root).map(plainUnit) : [];
}

export class NoteDiffState {
  /** The note and text the version was asked for. */
  private subjectKey: string | null = null;
  /** Bumped on every load, so a fetch that resolves late is dropped. */
  private fetchSeq = 0;
  private version = $state.raw<NoteVersion | null>(null);

  /** The previous version's HTML as last compared, and the blocks parsed from it. */
  private previousHtml: string | null = null;
  private parsedPrevious: NoteDiffUnit[] | null = null;
  /** The blocks the diff was computed from. */
  private comparedPrevious: NoteDiffUnit[] | null = null;
  private comparedCurrent: NoteDiffUnit[] | null = null;
  private computed = $state.raw<NoteDiff | null>(null);

  private hovered = $state<string | null>(null);
  private active = $state<string | null>(null);
  private split = $state(false);
  private aligns = $state(0);
  private follows = false;
  private measured = $state.raw<Record<NoteDiffSide, readonly NoteDiffUnitBox[]>>({
    previous: NO_BOXES,
    current: NO_BOXES,
  });

  /** The text the note held before its latest revision; `null` until fetched, or when there is none. */
  get previous(): NoteVersion | null {
    return this.version;
  }

  /** The diff of the two versions; `null` until both have been read. */
  get diff(): NoteDiff | null {
    return this.computed;
  }

  get hunks(): readonly NoteDiffHunk[] {
    return this.computed?.hunks ?? NO_HUNKS;
  }

  /** The previous version's blocks the diff was computed from. */
  get previousUnits(): readonly NoteDiffUnit[] | null {
    return this.comparedPrevious;
  }

  get hoveredHunkId(): string | null {
    return this.hovered;
  }

  /** The change that was selected, which stays lit. */
  get activeHunkId(): string | null {
    return this.active;
  }

  get activeHunk(): NoteDiffHunk | null {
    return this.hunk(this.active);
  }

  /** The change to show lit: the hovered one, else the selected one. */
  get pointedHunkId(): string | null {
    return this.hovered ?? this.active;
  }

  get pointedHunk(): NoteDiffHunk | null {
    return this.hunk(this.pointedHunkId);
  }

  /** The previous version is asked for beside the note. */
  get open(): boolean {
    return this.split;
  }

  /** Something of the diff is held open: what Escape lets go of before the note. */
  get engaged(): boolean {
    return this.split || this.active !== null;
  }

  /** Each side's blocks as last measured, by unit index (see `measureNoteDiffUnits`). */
  get boxes(): Record<NoteDiffSide, readonly NoteDiffUnitBox[]> {
    return this.measured;
  }

  /** Changes each time a change is selected: the note is to scroll to it. */
  get alignRequest(): number {
    return this.aligns;
  }

  /**
   * The note keeps the selected change on its reference line as the layout
   * moves under it (the second pane opening, an image loading), until the
   * reader scrolls.
   */
  get following(): boolean {
    return this.follows;
  }

  private hunk(id: string | null): NoteDiffHunk | null {
    if (id === null) return null;
    return this.hunks.find((hunk) => hunk.id === id) ?? null;
  }

  /**
   * Fetch the previous version of the note on screen, or let go of
   * everything with `null`.
   *
   * A version is the previous one of the text it was fetched for. When the
   * text is another (the note was revised, or another note swapped in) the
   * version held is dropped at once, rather than compared with text it does
   * not precede until the new one arrives. For the same text the version
   * held stays while it is fetched again, and through an answer that is the
   * same version: a write that left the text alone.
   */
  load(subject: NoteDiffSubject | null): void {
    const key = subject ? [subject.id, subject.title, subject.content].join('\u0000') : null;
    if (key !== this.subjectKey) {
      this.subjectKey = key;
      this.version = null;
    }
    const seq = ++this.fetchSeq;
    if (!subject) return;
    getNotePreviousVersion(subject.id).then(
      (version) => {
        if (seq !== this.fetchSeq) return;
        if (version?.id === this.version?.id) return;
        this.version = version;
      },
      (error) => {
        // Without it the note shows as it always has; the next load asks again.
        if (seq === this.fetchSeq) {
          console.warn('Failed to load the previous version of a note', error);
        }
      }
    );
  }

  /**
   * Compare the previous version, as its rendered HTML, with the current
   * one, as the blocks read from the rendered note. `null` for either means
   * it is not there to compare, and there is no diff.
   *
   * A new diff has new hunks, so nothing is pointed at any more; one with
   * no hunks has nothing to show beside the note either.
   */
  compare(previousHtml: string | null, current: readonly NoteDiffUnit[] | null): void {
    if (previousHtml !== this.previousHtml) {
      this.previousHtml = previousHtml;
      this.parsedPrevious = previousHtml === null ? null : parseNoteDiffUnits(previousHtml);
    }
    const previous = this.parsedPrevious;
    if (!previous || !current) {
      this.comparedPrevious = null;
      this.comparedCurrent = null;
      this.setDiff(null);
      return;
    }
    if (
      this.computed &&
      sameNoteDiffUnits(previous, this.comparedPrevious) &&
      sameNoteDiffUnits(current, this.comparedCurrent)
    ) {
      return;
    }
    this.comparedPrevious = previous;
    // Copies: the units read from the note hold its elements.
    this.comparedCurrent = current.map(plainUnit);
    this.setDiff(computeNoteDiff(previous, this.comparedCurrent));
  }

  private setDiff(diff: NoteDiff | null): void {
    if (diff === this.computed) return;
    this.computed = diff;
    this.hovered = null;
    this.active = null;
    this.follows = false;
    if (!diff || diff.hunks.length === 0) this.split = false;
  }

  /** The pointer is over a change's marker, or has left it (`null`). */
  hover(hunkId: string | null): void {
    this.hovered = hunkId;
  }

  /**
   * A change's marker was pressed. The change stays lit and the note
   * scrolls to it; with `split`, the previous version opens beside the
   * note. Without a second pane, pressing the lit change's marker again
   * lets go of it.
   */
  select(hunkId: string, split: boolean): void {
    if (!this.hunk(hunkId)) return;
    if (!split && !this.split && this.active === hunkId) {
      this.close();
      return;
    }
    this.active = hunkId;
    this.split = split;
    this.follows = true;
    this.aligns++;
  }

  /**
   * Close the previous version and let go of the selected change. A hovered
   * change is not let go of here: a marker that still has the pointer or
   * keyboard focus goes on showing its change. A hover whose element goes
   * with the pane is ended by whoever drew it (`ChangeHover` in
   * noteDiffPointer.ts).
   */
  close(): void {
    this.split = false;
    this.active = null;
    this.follows = false;
  }

  /** Record one side's measured blocks. The same measurements again change nothing. */
  setBoxes(side: NoteDiffSide, boxes: readonly NoteDiffUnitBox[]): void {
    if (sameBoxes(boxes, this.measured[side])) return;
    this.measured = { ...this.measured, [side]: boxes };
  }

  /**
   * Stop following the selected change once the reader scrolls `scroller`
   * themselves: a wheel, a finger, a key, or a press (the scrollbar's
   * included). Also given what scrolls a pane from outside it, the
   * connectors between the panes. Returns the undo.
   */
  watchReaderScroll(scroller: Element): () => void {
    const stop = () => {
      this.follows = false;
    };
    for (const type of READER_SCROLL_EVENTS) {
      scroller.addEventListener(type, stop, { passive: true });
    }
    return () => {
      for (const type of READER_SCROLL_EVENTS) scroller.removeEventListener(type, stop);
    };
  }
}
