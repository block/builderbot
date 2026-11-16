<!--
  NoteQuoteHighlights.svelte — the passages a note's reply draft quotes,
  marked in the rendered note.

  Renders nothing of its own: it wraps the quoted text of `contentEl` in
  highlight spans (see noteQuoteHighlights.ts) and keeps them in step with
  the draft's quotes (`quotes`) and with the note's markup, which is rebuilt
  from scratch on every render (`renderKey` is what it was rendered from) and
  has its marks taken out for each find-in-note pass, which searches whole
  text and leaves the marks to be put back around its matches (`refreshKey`).
  A quote leaving the draft — removed, or sent — loses its mark; one the
  note no longer holds the text of never gets one.

  Pressing a mark, or Enter or Space on its keyboard stop, asks for that
  quote's reply to be reopened (`onActivate`, with the passage's range to
  anchor the field to). A press that selected text is the start of a new
  quote and is left alone, as is a press on marked text that is also a link
  or a hashtag badge. `activeQuoteId` is the quote whose reply field is
  open; its mark stays lit, as a hovered one's does, across all its pieces.

  Every pass replaces all the spans, so what the old ones held is put on the
  new: the lit state of the open and the hovered quote, keyboard focus when
  a quote's stop had it, and a press that was under way. A browser sends a
  click only when the button comes up on the element it went down on, or on
  one still in the page around both; a span replaced between the two gets
  none. That happens when the press itself closes a reply field left empty:
  the field's quote leaves the draft a moment later and the marks are
  rebuilt under the pointer. So a press whose span has gone is answered as
  the button comes up, on the same quote's new span.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { DraftQuote } from '../sessions/composerDraft';
  import {
    applyQuoteHighlights,
    clearQuoteHighlights,
    quoteHighlightRange,
    quoteHighlightStop,
    quoteIdAtTarget,
    QUOTE_HIGHLIGHT_CLASS,
    setQuoteHighlightState,
  } from './noteQuoteHighlights';

  interface Props {
    /** The rendered note. */
    contentEl: HTMLElement | undefined;
    /** What the note was rendered from; a new render starts without marks. */
    renderKey: unknown;
    /** Bumped when something else rewrote the note's text nodes under the marks. */
    refreshKey?: unknown;
    /** The reply draft's quote blocks. */
    quotes: readonly DraftQuote[];
    /** The quote whose reply field is open. */
    activeQuoteId?: string | null;
    /** Headings left out of a quote's heading path (the note's title). */
    ignoreHeading?: (heading: Element) => boolean;
    /** The quotes that are marked in the note, after each pass. */
    onMarked?: (quoteIds: string[]) => void;
    /** A mark was pressed; `stop` is the quote's keyboard stop, when a key press on it did. */
    onActivate: (quoteId: string, range: Range, stop: HTMLElement | null) => void;
  }

  let {
    contentEl,
    renderKey,
    refreshKey = null,
    quotes,
    activeQuoteId = null,
    ignoreHeading,
    onMarked,
    onActivate,
  }: Props = $props();

  const ACTIVE_CLASS = 'is-active';
  const HOVERED_CLASS = 'is-hovered';
  /** How far the pointer may travel between going down and coming up and still be a press, not a drag. */
  const PRESS_SLOP_PX = 4;
  /** How soon after a press was answered a click is taken for that same press. */
  const ANSWERED_CLICK_MS = 500;

  /** The quote the pointer is over, whose mark is lit. */
  let hoveredQuoteId: string | null = null;
  /**
   * The quote whose keyboard stop has focus, or had it when the stop was
   * taken out of the note: a removed element gives up focus without an
   * event, so this is only let go when focus moves to something else.
   */
  let focusedQuoteId: string | null = null;

  /** The quote `target` is the keyboard stop of, if it is one. */
  function stopQuoteId(target: EventTarget | null): string | null {
    if (
      !(target instanceof HTMLElement) ||
      !target.classList.contains(QUOTE_HIGHLIGHT_CLASS) ||
      target.getAttribute('role') !== 'button'
    ) {
      return null;
    }
    return target.getAttribute('data-quote-id');
  }

  /**
   * Give focus back to the stop that lost it when the marks were rebuilt
   * (by this pass, a new render of the note, or a find-in-note pass). Only
   * while focus is where a removed element leaves it, on the page or on a
   * container around the note: anything else took it since and keeps it.
   */
  function restoreFocus(el: HTMLElement) {
    const quoteId = focusedQuoteId;
    if (quoteId === null) return;
    const active = el.ownerDocument.activeElement;
    const adrift = !active || active === el.ownerDocument.body || active.contains(el);
    const stop = adrift ? quoteHighlightStop(el, quoteId) : null;
    // Without scrolling: the reader may be elsewhere in the note by now.
    if (stop) stop.focus({ preventScroll: true });
    else focusedQuoteId = null;
  }

  // What decides where the marks go. A reply being typed changes `quotes`
  // every few hundred milliseconds and none of this, so it re-marks nothing.
  let locationKey = $derived(
    JSON.stringify(
      quotes.map((quote) => [quote.id, quote.quote, quote.headings, quote.anchor ?? null])
    )
  );

  $effect(() => {
    const el = contentEl;
    void renderKey;
    void refreshKey;
    void locationKey;
    if (!el) return;
    untrack(() => {
      const marked = applyQuoteHighlights(el, quotes, { ignore: ignoreHeading });
      setQuoteHighlightState(el, ACTIVE_CLASS, activeQuoteId);
      // The pointer has not moved, so no event will light the new spans
      // under it; if the note moved under it instead, the next one corrects this.
      setQuoteHighlightState(el, HOVERED_CLASS, hoveredQuoteId);
      restoreFocus(el);
      onMarked?.(marked);
    });
    return () => clearQuoteHighlights(el);
  });

  $effect(() => {
    const el = contentEl;
    const id = activeQuoteId;
    if (el) setQuoteHighlightState(el, ACTIVE_CLASS, id);
  });

  function activate(el: HTMLElement, quoteId: string, stop: HTMLElement | null) {
    const range = quoteHighlightRange(el, quoteId);
    if (range) onActivate(quoteId, range, stop);
  }

  $effect(() => {
    const el = contentEl;
    if (!el) return;
    // Another element starts with nothing under the pointer, whatever the
    // pass above lit in it for the one before.
    hoveredQuoteId = null;
    setQuoteHighlightState(el, HOVERED_CLASS, null);

    /** A drag across marked text selected it, for a quote of its own: no press. */
    const selectedText = () => {
      const selection = document.getSelection();
      return !!selection && !selection.isCollapsed;
    };

    /**
     * The press under way on a mark, from the pointer going down to its
     * coming up: the quote, the span pressed, and where.
     */
    let press: { quoteId: string; span: Node; pointerId: number; x: number; y: number } | null =
      null;
    /** When that press was answered as the pointer came up, ahead of any click. */
    let answeredAt: number | null = null;

    // On the document, so every press starts from nothing: one that ended
    // outside the note is not left to be matched with a later pointer-up.
    const handlePointerDown = (event: PointerEvent) => {
      press = null;
      answeredAt = null;
      if (event.button !== 0 || !(event.target instanceof Node)) return;
      const quoteId = quoteIdAtTarget(el, event.target);
      if (!quoteId) return;
      press = {
        quoteId,
        span: event.target,
        pointerId: event.pointerId,
        x: event.clientX,
        y: event.clientY,
      };
    };

    const handlePointerUp = (event: PointerEvent) => {
      const pressed = press;
      press = null;
      if (!pressed || event.pointerId !== pressed.pointerId || event.button !== 0) return;
      // The span is where it was: the browser's click follows.
      if (pressed.span.isConnected) return;
      // Replaced under the pointer, and no click will come (see the header).
      // Only the same quote, where the pointer went down: the press is not
      // passed on to whatever else the new marks put there, and a quote
      // that left the draft meanwhile has no mark to come up on.
      if (quoteIdAtTarget(el, event.target) !== pressed.quoteId) return;
      if (Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y) > PRESS_SLOP_PX) return;
      if (selectedText()) return;
      answeredAt = event.timeStamp;
      activate(el, pressed.quoteId, null);
    };

    const handlePointerCancel = () => {
      press = null;
    };

    const handleClick = (event: MouseEvent) => {
      // An engine that does send a click for a press answered already. Not
      // a click of its own some time later, which has no pointer-down to
      // start it afresh when a screen reader makes it.
      const sincePress = answeredAt === null ? null : event.timeStamp - answeredAt;
      answeredAt = null;
      if (sincePress !== null && sincePress < ANSWERED_CLICK_MS) return;
      // The second click of a double-click selects a word; it reopens nothing.
      if (event.defaultPrevented || event.button !== 0 || event.detail > 1) return;
      const quoteId = quoteIdAtTarget(el, event.target);
      if (!quoteId) return;
      if (selectedText()) return;
      activate(el, quoteId, null);
    };

    const handleKeydown = (event: KeyboardEvent) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      const quoteId = stopQuoteId(event.target);
      if (!quoteId) return;
      // Space would scroll the note, and the note's own key handler has
      // nothing to do with a mark.
      event.preventDefault();
      event.stopPropagation();
      activate(el, quoteId, event.target as HTMLElement);
    };

    const hover = (quoteId: string | null) => {
      if (quoteId === hoveredQuoteId) return;
      hoveredQuoteId = quoteId;
      setQuoteHighlightState(el, HOVERED_CLASS, quoteId);
    };
    // A finger has no hover: a tap's would stay lit until the next tap.
    const handlePointerOver = (event: PointerEvent) => {
      if (event.pointerType !== 'touch') hover(quoteIdAtTarget(el, event.target));
    };
    const handlePointerLeave = () => hover(null);

    const handleFocusIn = (event: FocusEvent) => {
      focusedQuoteId = stopQuoteId(event.target);
    };
    // Focus moved on to another element. With nothing taking it (the window
    // went to the background, or an engine reporting a removal) it is still
    // the stop's to get back.
    const handleFocusOut = (event: FocusEvent) => {
      if (event.relatedTarget) focusedQuoteId = null;
    };

    const doc = el.ownerDocument;
    doc.addEventListener('pointerdown', handlePointerDown, true);
    doc.addEventListener('pointerup', handlePointerUp, true);
    doc.addEventListener('pointercancel', handlePointerCancel, true);
    el.addEventListener('click', handleClick);
    el.addEventListener('keydown', handleKeydown);
    el.addEventListener('pointerover', handlePointerOver);
    el.addEventListener('pointerleave', handlePointerLeave);
    el.addEventListener('focusin', handleFocusIn);
    el.addEventListener('focusout', handleFocusOut);
    return () => {
      doc.removeEventListener('pointerdown', handlePointerDown, true);
      doc.removeEventListener('pointerup', handlePointerUp, true);
      doc.removeEventListener('pointercancel', handlePointerCancel, true);
      el.removeEventListener('click', handleClick);
      el.removeEventListener('keydown', handleKeydown);
      el.removeEventListener('pointerover', handlePointerOver);
      el.removeEventListener('pointerleave', handlePointerLeave);
      el.removeEventListener('focusin', handleFocusIn);
      el.removeEventListener('focusout', handleFocusOut);
    };
  });
</script>

<style>
  /* The note's own tint, as on the selection a reply is being written to and
     the rule beside each quote, with a line under it: the tint alone is
     faint in the light theme. Longhands for the line (see app.css on
     `mark.search-match`). A find-in-note match inside a mark draws its own,
     thicker line over this one. */
  :global(.note-quote-highlight) {
    border-radius: 2px;
    background-color: var(--note-bg-emphasis);
    text-decoration-line: underline;
    text-decoration-color: var(--note-color);
    text-decoration-thickness: 1px;
    text-underline-offset: 3px;
    text-decoration-skip-ink: none;
    -webkit-box-decoration-break: clone;
    box-decoration-break: clone;
    transition: background-color 0.1s;
  }

  /* Two quotes over the same words: their tints add up. */
  :global(.note-quote-highlight.is-overlap) {
    background-color: color-mix(in srgb, var(--note-color) 26%, transparent);
  }

  /* A mark that reopens its reply. One inside a link or a badge keeps that
     element's cursor and press. */
  :global(.note-quote-highlight[data-quote-id]) {
    cursor: pointer;
  }

  :global(.note-quote-highlight.is-hovered),
  :global(.note-quote-highlight.is-active) {
    background-color: color-mix(in srgb, var(--note-color) 34%, transparent);
    text-decoration-thickness: 2px;
  }

  :global(.note-quote-highlight:focus-visible) {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }

  @media (prefers-reduced-motion: reduce) {
    :global(.note-quote-highlight) {
      transition: none;
    }
  }
</style>
