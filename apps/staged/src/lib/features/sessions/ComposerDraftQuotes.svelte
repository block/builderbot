<!--
  ComposerDraftQuotes.svelte — The quote blocks of a reply draft, above the
  composer's main input, with the free text typed between them.

  The draft runs `text, quote, text, quote, …` and ends in the main input
  (see composerDraft.ts). Each gap before a quote is an input of its own that
  takes no room beyond the spacing between blocks: its placeholder shows on
  hover or focus, and it grows as text goes in. Backspace in an empty gap
  takes out the quote below it, the one the gap's text belongs to, as long
  as nothing has been written under it; a quote with a reply is kept and the
  caret moves to the end of that reply instead. The list scrolls once a
  review outgrows the pane, and keeps its last quote, the newest and the one
  beside the main input, in view as quotes arrive.
-->
<script lang="ts">
  import HashtagInput from './HashtagInput.svelte';
  import ComposerQuoteBlock from './ComposerQuoteBlock.svelte';
  import type { DraftQuote } from './composerDraft';
  import type { HashtagItem } from '../../types';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import { focusAtEndSync, focusAtStartSync } from '../../shared/focusAtEnd';

  interface Props {
    quotes: DraftQuote[];
    hashtagItems?: HashtagItem[];
    onkeydown?: (e: KeyboardEvent) => void;
    oninput?: (e: Event) => void;
    /** An IME or dictation composition is open in a gap or a quote's reply. */
    composing?: boolean;
    /** That composition ended (committed or cancelled). */
    oncompositionend?: () => void;
    onRemoveQuote: (id: string) => void;
  }

  let {
    quotes = $bindable(),
    hashtagItems = [],
    onkeydown,
    oninput,
    composing = $bindable(false),
    oncompositionend,
    onRemoveQuote,
  }: Props = $props();

  let listEl = $state<HTMLDivElement>();
  // From nothing, so the first run — the list mounting with a restored
  // draft, or with the quote that started it — counts as growth too.
  let shownCount = 0;

  // A quote added at the end scrolls into view, and so does the end of a
  // draft restored with quotes in it; edits and removals don't move the list.
  $effect(() => {
    const count = quotes.length;
    const grew = count > shownCount;
    shownCount = count;
    if (!grew || !listEl) return;
    const cards = listEl.querySelectorAll<HTMLElement>('.composer-quote');
    cards[cards.length - 1]?.scrollIntoView({ block: 'nearest' });
  });

  /** The input holding an open composition: `gap:<id>` or `reply:<id>`. */
  let composingInput = $state<string | null>(null);

  function setInputComposing(key: string, value: boolean) {
    if (value) composingInput = key;
    else if (composingInput === key) composingInput = null;
    composing = composingInput !== null;
  }

  // An input taken away mid-composition (its quote removed, or the draft
  // replaced) sends no end; don't leave the composer waiting on one.
  $effect(() => {
    const key = composingInput;
    if (key === null) return;
    const id = key.slice(key.indexOf(':') + 1);
    if (!quotes.some((quote) => quote.id === id)) setInputComposing(key, false);
  });

  /**
   * Backspace with nothing left in a gap removes the quote below it: the gap
   * is that quote's leading text. Only an unanswered quote goes, though, as
   * there is no undo: one with a reply stays, and the caret moves to the end
   * of the reply, the way Backspace at the start of a block joins the one
   * above. A held key's repeats do neither, so holding Backspace to clear
   * the gap stops once it's empty.
   */
  function handleGapKeydown(event: KeyboardEvent, quote: DraftQuote) {
    if (
      event.key === 'Backspace' &&
      quote.before === '' &&
      !event.repeat &&
      !event.shiftKey &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey &&
      composingInput === null &&
      !isImeKeydown(event)
    ) {
      event.preventDefault();
      if (quote.reply.trim() === '') onRemoveQuote(quote.id);
      else focusAtEndSync(replyFor(quote.id));
      return;
    }
    onkeydown?.(event);
  }

  function entryFor(id: string): HTMLElement | undefined {
    if (!listEl) return undefined;
    return [...listEl.querySelectorAll<HTMLElement>('[data-quote-entry]')].find(
      (entry) => entry.dataset.quoteEntry === id
    );
  }

  function replyFor(id: string): HTMLElement | null {
    return entryFor(id)?.querySelector<HTMLElement>('.composer-quote-reply') ?? null;
  }

  /** Whether focus is in a quote's block or the gap before it. */
  export function holdsFocus(id: string): boolean {
    const active = document.activeElement;
    return !!active && !!entryFor(id)?.contains(active);
  }

  /** Focus a quote's reply with the caret at its end, for a quote just added from the note. */
  export function focusReply(id: string): boolean {
    const reply = replyFor(id);
    if (!reply) return false;
    focusAtEndSync(reply);
    return true;
  }

  /** Focus the gap before a quote, with the caret at its start or end. */
  export function focusGap(id: string, caret: 'start' | 'end'): boolean {
    const gap = entryFor(id)?.querySelector<HTMLElement>('.composer-gap-input') ?? null;
    if (!gap) return false;
    if (caret === 'start') focusAtStartSync(gap);
    else focusAtEndSync(gap);
    return true;
  }
</script>

<div class="composer-draft-quotes" bind:this={listEl} role="group" aria-label="Quoted replies">
  {#each quotes as quote, index (quote.id)}
    <div class="composer-draft-entry" data-quote-entry={quote.id}>
      <HashtagInput
        bind:value={quotes[index].before}
        bind:composing={
          () => composingInput === `gap:${quote.id}`,
          (value) => setInputComposing(`gap:${quote.id}`, value)
        }
        {oncompositionend}
        class="composer-gap-input"
        placeholder={index === 0 ? 'Add a message…' : 'Add text between quotes…'}
        rows={1}
        items={hashtagItems}
        onkeydown={(event) => handleGapKeydown(event, quote)}
        {oninput}
      />
      <ComposerQuoteBlock
        bind:quote={quotes[index]}
        bind:composing={
          () => composingInput === `reply:${quote.id}`,
          (value) => setInputComposing(`reply:${quote.id}`, value)
        }
        {oncompositionend}
        {hashtagItems}
        {onkeydown}
        {oninput}
        onRemove={() => onRemoveQuote(quote.id)}
      />
    </div>
  {/each}
</div>

<style>
  .composer-draft-quotes {
    display: flex;
    flex-direction: column;
    max-height: min(45vh, 420px);
    overflow-y: auto;
    overscroll-behavior: contain;
    /* Room for the cards' focus border at the scroll edges. */
    padding: 1px;
    margin: -1px;
  }

  /* A gap and its quote, kept together for focus and lookup; no box of its own. */
  .composer-draft-entry {
    display: contents;
  }

  .composer-draft-quotes :global(.composer-gap-input) {
    padding: 3px 4px;
    background: transparent;
    border: none;
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
  }

  /* An empty gap is just the space between blocks until it is pointed at. */
  .composer-draft-quotes :global(.composer-gap-input.is-empty) {
    --hashtag-input-min-height: 8px;
    padding-block: 2px;
    line-height: 1;
    cursor: text;
  }

  .composer-draft-quotes :global(.composer-gap-input.is-empty::before) {
    opacity: 0;
    transition: opacity 0.1s;
  }

  .composer-draft-quotes :global(.composer-gap-input.is-empty:hover),
  .composer-draft-quotes :global(.composer-gap-input:focus) {
    --hashtag-input-min-height: 1.5em;
    padding-block: 3px;
    line-height: 1.5;
    background: var(--bg-hover);
  }

  .composer-draft-quotes :global(.composer-gap-input.is-empty:hover::before),
  .composer-draft-quotes :global(.composer-gap-input:focus::before) {
    opacity: 1;
  }

  @media (hover: none), (pointer: coarse) {
    /* No hover to reveal a gap: each one is a finger-sized row (44px) with
       its placeholder showing, so text between quotes has a visible place. */
    .composer-draft-quotes :global(.composer-gap-input) {
      --hashtag-input-min-height: 1.5em;
      padding: 12px 8px;
      line-height: 1.5;
    }

    .composer-draft-quotes :global(.composer-gap-input.is-empty) {
      --hashtag-input-min-height: 1.5em;
      padding-block: 12px;
      line-height: 1.5;
    }

    .composer-draft-quotes :global(.composer-gap-input.is-empty::before) {
      opacity: 0.6;
    }
  }
</style>
