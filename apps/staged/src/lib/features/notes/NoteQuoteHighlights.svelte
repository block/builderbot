<!--
  NoteQuoteHighlights.svelte — the passages a note's reply draft quotes,
  marked in the rendered note.

  Renders nothing of its own: it wraps the quoted text of `contentEl` in
  highlight spans (see noteQuoteHighlights.ts) and keeps them in step with
  the draft's quotes (`quotes`) and with the note's markup, which is rebuilt
  from scratch on every render (`renderKey` is what it was rendered from) and
  flattened wherever a find-in-note match is cleared (`refreshKey`). A quote
  leaving the draft — removed, or sent — loses its mark; one the note no
  longer holds the text of never gets one.

  Pressing a mark, or Enter or Space on its keyboard stop, asks for that
  quote's reply to be reopened (`onActivate`, with the passage's range to
  anchor the field to). A press that selected text is the start of a new
  quote and is left alone, as is a press on marked text that is also a link
  or a hashtag badge. `activeQuoteId` is the quote whose reply field is
  open; its mark stays lit, as a hovered one's does, across all its pieces.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import type { DraftQuote } from '../sessions/composerDraft';
  import {
    applyQuoteHighlights,
    clearQuoteHighlights,
    quoteHighlightRange,
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
    /** A mark was pressed; `stop` is the element to return focus to, for a key press. */
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
    let hovered: string | null = null;

    const handleClick = (event: MouseEvent) => {
      // The second click of a double-click selects a word; it reopens nothing.
      if (event.defaultPrevented || event.button !== 0 || event.detail > 1) return;
      const quoteId = quoteIdAtTarget(el, event.target);
      if (!quoteId) return;
      // A drag across marked text selected it, for a quote of its own.
      const selection = document.getSelection();
      if (selection && !selection.isCollapsed) return;
      activate(el, quoteId, null);
    };

    const handleKeydown = (event: KeyboardEvent) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      const stop = event.target;
      if (
        !(stop instanceof HTMLElement) ||
        !stop.classList.contains(QUOTE_HIGHLIGHT_CLASS) ||
        stop.getAttribute('role') !== 'button'
      ) {
        return;
      }
      const quoteId = stop.getAttribute('data-quote-id');
      if (!quoteId) return;
      // Space would scroll the note, and the note's own key handler has
      // nothing to do with a mark.
      event.preventDefault();
      event.stopPropagation();
      activate(el, quoteId, stop);
    };

    const hover = (quoteId: string | null) => {
      if (quoteId === hovered) return;
      hovered = quoteId;
      setQuoteHighlightState(el, HOVERED_CLASS, quoteId);
    };
    // A finger has no hover: a tap's would stay lit until the next tap.
    const handlePointerOver = (event: PointerEvent) => {
      if (event.pointerType !== 'touch') hover(quoteIdAtTarget(el, event.target));
    };
    const handlePointerLeave = () => hover(null);

    el.addEventListener('click', handleClick);
    el.addEventListener('keydown', handleKeydown);
    el.addEventListener('pointerover', handlePointerOver);
    el.addEventListener('pointerleave', handlePointerLeave);
    return () => {
      el.removeEventListener('click', handleClick);
      el.removeEventListener('keydown', handleKeydown);
      el.removeEventListener('pointerover', handlePointerOver);
      el.removeEventListener('pointerleave', handlePointerLeave);
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
