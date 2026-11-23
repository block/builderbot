<!--
  NotePreviousPane.svelte — a note's previous version, beside the note.

  Read-only: the version is shown as the note is, with no search, no reply
  quotes and no selection reply. The viewer renders the version itself
  (`content`), so it is styled and handles presses exactly as the note
  does, and hands over the rendered element (`contentEl`; `renderKey` is
  what it was rendered from).

  Its blocks are marked from the viewer's diff (`noteDiff`): the hunk being
  pointed at lit, with the words it lost, the others tinted, and a line
  where the revision added text this version does not have. The diff was
  computed from this version's HTML parsed off screen; the blocks read here
  are checked against those, and left unmarked if they differ.

  The pane scrolls with the note (`currentScrollEl`). Each maps the other's
  place through the blocks both versions hold (noteDiffScroll.ts), so a
  shared block on the reference line in one is on it in the other. The pane
  the reader last scrolled leads; when either is laid out again, the other
  is put back in step with it. A scroll this pane caused is not answered,
  or the two would chase each other through the map's rounding.

  The two scroll areas have to be the same box for that line to be level:
  the note's pane carries the same header, and `bottomInset` is the height
  of what stands under the note's scroll area, kept clear here too.
-->
<script lang="ts">
  import { untrack, type Snippet } from 'svelte';
  import { formatRelativeTime } from '../../shared/relativeTime';
  import type { NoteDiffSide } from './noteDiff';
  import {
    applyNoteDiffMarks,
    clearNoteDiffMarks,
    pointAtNoteDiffHunk,
  } from './noteDiffHighlights';
  import { buildScrollAnchors, syncedScrollTop } from './noteDiffScroll';
  import { sameNoteDiffUnits, type NoteDiffState } from './noteDiffState.svelte';
  import {
    extractNoteDiffUnits,
    measureNoteDiffUnits,
    type NoteDiffDomUnit,
  } from './noteDiffUnits';
  import NoteDiffGaps from './NoteDiffGaps.svelte';
  import NoteDiffPaneHeader from './NoteDiffPaneHeader.svelte';

  interface Props {
    noteDiff: NoteDiffState;
    /** The rendered previous version: the element `content` renders. */
    contentEl: HTMLElement | undefined;
    /** What it was rendered from; a new render has new blocks. */
    renderKey: unknown;
    /** When the version's text was written (ms). */
    savedAt: number;
    /** The element that scrolls the note. */
    currentScrollEl: HTMLElement | undefined;
    /** The height of what stands under the note's scroll area. */
    bottomInset?: number;
    onClose: () => void;
    /** A press or a key in the version, handled as the note's are. */
    onContentClick?: (event: MouseEvent) => void;
    onContentKeydown?: (event: KeyboardEvent) => void;
    /** The rendered version. */
    content: Snippet;
  }

  let {
    noteDiff,
    contentEl,
    renderKey,
    savedAt,
    currentScrollEl,
    bottomInset = 0,
    onClose,
    onContentClick,
    onContentKeydown,
    content,
  }: Props = $props();

  let scrollEl = $state<HTMLElement>();
  /** The version's blocks as last read; none when they are not the ones the diff compared. */
  let units = $state.raw<NoteDiffDomUnit[]>([]);

  $effect(() => {
    const el = contentEl;
    void renderKey;
    const hunks = noteDiff.hunks;
    if (!el) {
      units = [];
      return;
    }
    untrack(() => {
      const read = extractNoteDiffUnits(el);
      units = sameNoteDiffUnits(read, noteDiff.previousUnits) ? read : [];
      applyNoteDiffMarks(el, units, hunks, 'previous');
    });
    return () => clearNoteDiffMarks(el);
  });

  $effect(() => {
    const el = contentEl;
    const hunk = noteDiff.pointedHunk;
    const previous = units;
    if (!el) return;
    return untrack(() => pointAtNoteDiffHunk(el, previous, hunk, 'previous'));
  });

  // Measure the blocks, now and whenever the version is laid out again.
  $effect(() => {
    const el = contentEl;
    const scroller = scrollEl;
    const previous = units;
    if (!el || !scroller) return;

    const measure = () => noteDiff.setBoxes('previous', measureNoteDiffUnits(previous, scroller));
    untrack(measure);
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
    observer?.observe(el);
    observer?.observe(scroller);
    return () => {
      observer?.disconnect();
      noteDiff.setBoxes('previous', []);
    };
  });

  let anchors = $derived(
    noteDiff.diff
      ? buildScrollAnchors(noteDiff.diff.ops, noteDiff.boxes.previous, noteDiff.boxes.current)
      : []
  );

  /** The pane the reader last scrolled, which the other follows. */
  let leader: NoteDiffSide = 'current';
  /** Where this pane last put each scroll area, until the scroll event for it arrives. */
  const echoes = new Map<HTMLElement, number>();

  $effect(() => {
    const current = currentScrollEl;
    const previous = scrollEl;
    const map = anchors;
    if (!current || !previous) return;
    const panes = { current, previous };

    const follow = (from: NoteDiffSide) => {
      const source = panes[from];
      const target = panes[from === 'current' ? 'previous' : 'current'];
      const before = target.scrollTop;
      target.scrollTop = syncedScrollTop(map, from, source, target);
      // Read back: the pane may have rounded it, or have no further to go.
      if (target.scrollTop !== before) echoes.set(target, target.scrollTop);
    };
    const scrolled = (side: NoteDiffSide) => () => {
      const pane = panes[side];
      const echo = echoes.get(pane);
      echoes.delete(pane);
      if (echo !== undefined && Math.abs(pane.scrollTop - echo) < 1) return;
      leader = side;
      follow(side);
    };

    // The panes were laid out again, or this one has just opened.
    follow(leader);
    const onCurrent = scrolled('current');
    const onPrevious = scrolled('previous');
    current.addEventListener('scroll', onCurrent, { passive: true });
    previous.addEventListener('scroll', onPrevious, { passive: true });
    return () => {
      current.removeEventListener('scroll', onCurrent);
      previous.removeEventListener('scroll', onPrevious);
    };
  });

  $effect(() => {
    if (scrollEl) return noteDiff.watchReaderScroll(scrollEl);
  });
</script>

<section class="previous-pane" aria-label="Previous version">
  <NoteDiffPaneHeader
    label="Previous version"
    detail="saved {formatRelativeTime(savedAt)}"
    closeLabel="Close previous version"
    {onClose}
  />
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="previous-scroll note-diff-all"
    bind:this={scrollEl}
    onclick={onContentClick}
    onkeydown={onContentKeydown}
  >
    {@render content()}
    <NoteDiffGaps {noteDiff} side="previous" showAll />
  </div>
  {#if bottomInset > 0}
    <div class="previous-bottom" style:height="{bottomInset}px"></div>
  {/if}
</section>

<style>
  .previous-pane {
    display: flex;
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
    flex-direction: column;
    border-right: 1px solid var(--border-subtle);
  }

  /* The note's scroll area over again (`.modal-content` in NoteModal),
     positioned for the gap lines. The room under the text lets the last
     change reach the reference line, a third of the way down; the note's
     pane has the same. */
  .previous-scroll {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 24px 24px 60vh;
    background: var(--bg-primary);
  }

  .previous-scroll::-webkit-scrollbar {
    width: 6px;
  }

  .previous-scroll::-webkit-scrollbar-track {
    background: transparent;
  }

  .previous-scroll::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 3px;
  }

  .previous-scroll::-webkit-scrollbar-thumb:hover {
    background: var(--scrollbar-thumb-hover-transparent);
  }

  .previous-bottom {
    flex-shrink: 0;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
  }
</style>
