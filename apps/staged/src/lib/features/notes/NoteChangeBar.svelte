<!--
  NoteChangeBar.svelte — what a note's latest revision changed, as markers
  beside the note.

  One marker per hunk of the diff against the previous version (noteDiff.ts),
  each level with its change and as tall as it, in the note's right padding
  (NoteDiffMarkers.svelte draws them). Mounted inside the element that
  scrolls the note, which must be positioned: the markers are part of what
  it scrolls, and move with the text.

  Hovering a marker, or moving keyboard focus to it, lights its change in
  the note; pressing it selects the change (`onSelect`), which scrolls the
  note to it. What selecting opens is the viewer's to decide.

  The bar is also where the diff is made, since it is the one piece that is
  there whenever the note is. After each render of the note (`renderKey` is
  what it was rendered from) it reads the note's blocks, has them compared
  with the previous version's (`previousHtml`), and marks the changed ones
  (noteDiffHighlights.ts); it measures them, again whenever the note's
  layout changes, and the markers follow the measurements. With no previous
  version, or one that reads the same, there are no markers and nothing is
  rendered.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import {
    applyNoteDiffMarks,
    clearNoteDiffMarks,
    pointAtNoteDiffHunk,
  } from './noteDiffHighlights';
  import { hunkLead, scrollTopForOffset } from './noteDiffScroll';
  import type { NoteDiffState } from './noteDiffState.svelte';
  import {
    extractNoteDiffUnits,
    measureNoteDiffUnits,
    type NoteDiffDomUnit,
  } from './noteDiffUnits';
  import NoteDiffMarkers from './NoteDiffMarkers.svelte';

  interface Props {
    noteDiff: NoteDiffState;
    /** The rendered note. */
    contentEl: HTMLElement | undefined;
    /** The element that scrolls it. */
    scrollEl: HTMLElement | undefined;
    /** What the note was rendered from; a new render has new blocks. */
    renderKey: unknown;
    /** The previous version, rendered as the note is; `null` when there is none (yet). */
    previousHtml: string | null;
    /** A marker was pressed. */
    onSelect: (hunkId: string) => void;
  }

  let { noteDiff, contentEl, scrollEl, renderKey, previousHtml, onSelect }: Props = $props();

  /** The note's blocks as last read. */
  let units = $state.raw<NoteDiffDomUnit[]>([]);

  // Read the note's blocks, compare, and mark the changed ones. The markup
  // is rebuilt on every render, so its marks go with it.
  $effect(() => {
    const el = contentEl;
    const html = previousHtml;
    void renderKey;
    if (!el) {
      untrack(() => {
        units = [];
        noteDiff.compare(html, null);
      });
      return;
    }
    untrack(() => {
      const read = extractNoteDiffUnits(el);
      units = read;
      noteDiff.compare(html, read);
      applyNoteDiffMarks(el, read, noteDiff.hunks, 'current');
    });
    return () => clearNoteDiffMarks(el);
  });

  $effect(() => {
    const el = contentEl;
    const hunk = noteDiff.pointedHunk;
    const current = units;
    if (!el) return;
    return untrack(() => pointAtNoteDiffHunk(el, current, hunk, 'current'));
  });

  /**
   * Put the selected change on the note's reference line. One that starts
   * with text only the previous version has is that version's pane to
   * place, while it is shown (see `hunkLead`): the note follows it there.
   */
  function alignActive() {
    const scroller = scrollEl;
    const hunk = noteDiff.activeHunk;
    if (!scroller || !hunk) return;
    const lead = hunkLead(hunk, noteDiff.boxes);
    if (lead?.side === 'current') scroller.scrollTop = scrollTopForOffset(lead.offset, scroller);
  }

  // Measure the blocks, now and whenever the note is laid out again: an
  // image loading, the dialog resized, text wrapped again. The markers and
  // the gap lines are placed from these. Opening the previous version
  // changes how far the note scrolls without resizing anything observed, so
  // the selected change is put back on its line then too. A diff with no
  // changes has nothing to place.
  $effect(() => {
    const el = contentEl;
    const scroller = scrollEl;
    const current = units;
    void noteDiff.open;
    if (!el || !scroller || noteDiff.hunks.length === 0) return;

    const measure = () => {
      noteDiff.setBoxes('current', measureNoteDiffUnits(current, scroller));
      if (noteDiff.following) alignActive();
    };
    untrack(measure);
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    observer.observe(scroller);
    return () => observer.disconnect();
  });

  $effect(() => {
    void noteDiff.alignRequest;
    untrack(alignActive);
  });

  $effect(() => {
    if (scrollEl) return noteDiff.watchReaderScroll(scrollEl);
  });
</script>

<NoteDiffMarkers {noteDiff} side="current" label="Changes since the previous version" {onSelect} />
