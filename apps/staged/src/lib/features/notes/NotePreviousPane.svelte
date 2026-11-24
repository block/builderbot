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

  It carries the diff's markers too, level with this version's blocks: text
  the revision removed has its full height here, where the note has only a
  line. They are a second set of the changes the note's markers list, so
  they take presses (`onSelect`) and hovers but no Tab stop.

  Each change is joined to its place in the note by a band across the gutter
  between the panes (NoteDiffConnectors.svelte), from this pane's markers to
  the note's blocks. This pane knows how far both panes are scrolled, since
  it keeps them in step, so the bands are placed from here (`scrollTops`).
  Nothing stands between the markers and the bands: this pane's scrollbar is
  on its outer edge, the left, as the code diff viewer's before pane has it,
  and the line dividing the panes stops at their headers.

  The pane scrolls with the note (`currentScrollEl`). Each maps the other's
  place through the blocks both versions hold (noteDiffScroll.ts), so a
  shared block on the reference line in one is on it in the other. The pane
  the reader last scrolled leads; when either is laid out again, the other
  is put back in step with it. A scroll this pane caused is not answered,
  or the two would chase each other through the map's rounding.

  A selected change is scrolled to by the pane that has its first block.
  That is the note, through its change bar, except for a change that starts
  with text only this version has: the note has a gap there, which the map
  cannot tell from the block after it, so this pane puts the text's start on
  its own line and the note follows (`hunkLead`).

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
  import {
    buildScrollAnchors,
    hunkLead,
    scrollTopForOffset,
    syncedScrollTop,
  } from './noteDiffScroll';
  import { sameNoteDiffUnits, type NoteDiffState } from './noteDiffState.svelte';
  import {
    extractNoteDiffUnits,
    measureNoteDiffUnits,
    type NoteDiffDomUnit,
  } from './noteDiffUnits';
  import NoteDiffConnectors from './NoteDiffConnectors.svelte';
  import NoteDiffGaps from './NoteDiffGaps.svelte';
  import NoteDiffMarkers from './NoteDiffMarkers.svelte';
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
    /** A marker beside the version was pressed. */
    onSelect: (hunkId: string) => void;
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
    onSelect,
    onContentClick,
    onContentKeydown,
    content,
  }: Props = $props();

  let scrollEl = $state<HTMLElement>();
  /** The scroll area's visible height as last measured: the reference line moves with it. */
  let paneHeight = $state(0);
  /** The version's blocks as last read; none when they are not the ones the diff compared. */
  let units = $state.raw<NoteDiffDomUnit[]>([]);
  /**
   * How far each pane is scrolled, read once this pane has answered a
   * scroll: the connectors are drawn from the two together, so one read
   * before the other pane was moved would bend them for a frame.
   */
  let scrollTops = $state.raw<Record<NoteDiffSide, number>>({ previous: 0, current: 0 });

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

    const measure = () => {
      noteDiff.setBoxes('previous', measureNoteDiffUnits(previous, scroller));
      paneHeight = scroller.clientHeight;
    };
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

  /** The pane the reader last scrolled, or a selected change was placed from, which the other follows. */
  let leader: NoteDiffSide = 'current';
  /** Where this pane last put each scroll area, until the scroll event for it arrives. */
  const echoes = new Map<HTMLElement, number>();

  $effect(() => {
    const current = currentScrollEl;
    const previous = scrollEl;
    const map = anchors;
    // A change was selected, which may be this pane's to place, or the panes
    // changed height under the same blocks.
    void noteDiff.alignRequest;
    void paneHeight;
    if (!current || !previous) return;
    const panes = { current, previous };

    const scrollPane = (pane: HTMLElement, top: number) => {
      pane.scrollTop = top;
      // Read back: the pane may have rounded it, or have no further to go.
      // Noted even when the pane was there already. The event for an earlier
      // move may still be on its way (the change bar's, placing a change
      // before this pane was measured), and one that reports the place this
      // pane wants says nothing new.
      echoes.set(pane, pane.scrollTop);
    };
    const follow = (from: NoteDiffSide) => {
      const source = panes[from];
      const target = panes[from === 'current' ? 'previous' : 'current'];
      scrollPane(target, syncedScrollTop(map, from, source, target));
    };
    const readScrollTops = () => {
      const now = { previous: previous.scrollTop, current: current.scrollTop };
      if (now.previous !== scrollTops.previous || now.current !== scrollTops.current) {
        scrollTops = now;
      }
    };
    const scrolled = (side: NoteDiffSide) => () => {
      const pane = panes[side];
      const echo = echoes.get(pane);
      echoes.delete(pane);
      if (echo === undefined || Math.abs(pane.scrollTop - echo) >= 1) {
        leader = side;
        follow(side);
      }
      readScrollTops();
    };

    // The panes were laid out again, this one has just opened, or a change
    // was selected. While the selected change is being followed, the pane it
    // is placed from leads (see `hunkLead`): the note's change bar puts a
    // change on the note's line, and one that starts with text only this
    // version has is put on this pane's line here.
    untrack(() => {
      const hunk = noteDiff.following ? noteDiff.activeHunk : null;
      const lead = hunk ? hunkLead(hunk, noteDiff.boxes) : null;
      if (lead) leader = lead.side;
      if (lead?.side === 'previous') {
        scrollPane(previous, scrollTopForOffset(lead.offset, previous));
      }
      follow(leader);
      readScrollTops();
    });
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

  /**
   * The wheel was turned over a connector, which is in neither scroll area.
   * The note takes it, and this pane follows as it does any scroll of the
   * note's.
   */
  function scrollFromGutter(deltaY: number) {
    if (currentScrollEl) currentScrollEl.scrollTop += deltaY;
  }
</script>

<section class="previous-pane" aria-label="Previous version">
  <NoteDiffPaneHeader
    label="Previous version"
    detail="saved {formatRelativeTime(savedAt)}"
    closeLabel="Close previous version"
    {onClose}
  />
  <div class="previous-body">
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="previous-scroll note-diff-all"
      bind:this={scrollEl}
      onclick={onContentClick}
      onkeydown={onContentKeydown}
    >
      <div class="previous-content">
        {@render content()}
      </div>
      <NoteDiffGaps {noteDiff} side="previous" showAll />
      <NoteDiffMarkers
        {noteDiff}
        side="previous"
        label="Changes in the previous version"
        tabbable={false}
        {onSelect}
      />
    </div>
    <NoteDiffConnectors
      {noteDiff}
      {scrollTops}
      height={paneHeight}
      {onSelect}
      onWheel={scrollFromGutter}
    />
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
  }

  /* The line between the panes runs beside the header and beside what stands
     under the scroll area. Between them is the gutter the connectors cross,
     which it would cut in two. */
  .previous-pane > :global(.note-diff-pane-header),
  .previous-bottom {
    border-right: 1px solid var(--border-subtle);
  }

  /* The scroll area's box, positioned for the connectors, which lie over its
     right edge and are as tall as it. */
  .previous-body {
    position: relative;
    display: flex;
    flex: 1;
    min-height: 0;
    flex-direction: column;
  }

  /* The note's scroll area over again (`.modal-content` in NoteModal),
     positioned for the gap lines and the markers. The room under the text lets the last
     change reach the reference line, a third of the way down; the note's
     pane has the same.

     Its scrollbar is on the left, the pane's outer edge, so that the markers
     at its right edge meet the connectors: a scroll area laid out right to
     left has it there. What it holds reads left to right as ever.

     Laid out right to left, it also scrolls from its right edge, and what
     runs past that edge is out of reach: it scrolls up and down only. */
  .previous-scroll {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
    padding: 24px 24px 60vh;
    background: var(--bg-primary);
    direction: rtl;
  }

  .previous-scroll > :global(*) {
    direction: ltr;
  }

  /* The version itself, which scrolls sideways when something in it is wider
     than the pane (a table, a long path or hash that cannot wrap), as the
     note does in its own scroll area. The text is laid out to the pane's
     width either way, so both versions wrap alike. Reading left to right,
     it scrolls from its left edge. The markers and the gap lines are beside
     it, not in it, and stay where they are.

     A scroll box cuts off what is drawn outside it, as a tinted block's
     outline is (3px, see noteDiffStyles.css). So the box is larger than the
     text by that and a pixel all round, and pulled out by as much, which
     leaves the text where it would be without the box. */
  .previous-content {
    margin: -4px;
    padding: 4px;
    overflow-x: auto;
  }

  .previous-content::-webkit-scrollbar {
    height: 6px;
  }

  .previous-content::-webkit-scrollbar-track {
    background: transparent;
  }

  .previous-content::-webkit-scrollbar-thumb {
    background: var(--scrollbar-thumb-transparent);
    border-radius: 3px;
  }

  .previous-content::-webkit-scrollbar-thumb:hover {
    background: var(--scrollbar-thumb-hover-transparent);
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
