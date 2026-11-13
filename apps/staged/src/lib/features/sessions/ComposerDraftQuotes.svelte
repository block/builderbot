<!--
  ComposerDraftQuotes.svelte — The quote blocks of a reply draft, embedded in
  the composer's field above its main input, with the free text typed between
  them.

  The draft runs `text, quote, text, quote, …` and ends in the main input
  (see composerDraft.ts). Each gap before a quote is an input of its own, but
  the field around it draws the box, so the gap is borderless text in the
  same column as the main input and the segments read as one document: an
  empty gap is a thin strip the caret can be put in (it widens to a line
  while it has focus), and it grows as text goes in. On a phone, where there
  is no pointer to find the strip with, each gap is a finger-sized row with
  a faint hint in it instead. Backspace in an empty
  gap takes out the quote below it, the one the gap's text belongs to, as
  long as nothing has been written under it; a quote with a reply is kept
  and the caret moves to the end of that reply instead. The field is the
  scroll box; this list keeps its last quote, the newest and the one beside
  the main input, in view as quotes arrive.
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
    /**
     * A caret was placed by hand in one of the inputs here (Backspace moving
     * into a reply, a press on a block), for the field to bring into view.
     */
    onCaretPlaced?: () => void;
  }

  let {
    quotes = $bindable(),
    hashtagItems = [],
    onkeydown,
    oninput,
    composing = $bindable(false),
    oncompositionend,
    onRemoveQuote,
    onCaretPlaced,
  }: Props = $props();

  let listEl = $state<HTMLDivElement>();
  /** The field reveals the input itself; the browser's scroll-on-focus would move the page too. */
  const noScroll = { preventScroll: true } as const;
  // From nothing, so the first run — the list mounting with a restored
  // draft, or with the quote that started it — counts as growth too.
  let shownCount = 0;

  // A quote added at the end scrolls into view, and so does the end of a
  // draft restored with quotes in it; edits and removals don't move the field.
  $effect(() => {
    const count = quotes.length;
    const grew = count > shownCount;
    shownCount = count;
    if (!grew || !listEl) return;
    const blocks = listEl.querySelectorAll<HTMLElement>('.composer-quote');
    blocks[blocks.length - 1]?.scrollIntoView({ block: 'nearest' });
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
   * is that quote's leading text. Only an unanswered quote goes, though: a
   * key meant to rub out text shouldn't take a written reply with it, so one
   * with a reply stays, and the caret moves to the end of the reply, the way
   * Backspace at the start of a block joins the one above. A held key's
   * repeats do neither, so holding Backspace to clear the gap stops once
   * it's empty.
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
      else {
        focusAtEndSync(replyFor(quote.id), noScroll);
        onCaretPlaced?.();
      }
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

  /** A quote's block — breadcrumb, excerpt, reply — as rendered, for the field to scroll to. */
  export function blockFor(id: string): HTMLElement | null {
    return entryFor(id)?.querySelector<HTMLElement>('.composer-quote') ?? null;
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
    focusAtEndSync(reply, noScroll);
    return true;
  }

  /** Focus the gap before a quote, with the caret at its start or end. */
  export function focusGap(id: string, caret: 'start' | 'end'): boolean {
    const gap = entryFor(id)?.querySelector<HTMLElement>('.composer-gap-input') ?? null;
    if (!gap) return false;
    if (caret === 'start') focusAtStartSync(gap, noScroll);
    else focusAtEndSync(gap, noScroll);
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
        placeholder={index === 0 ? 'Add text before the quote' : 'Add text between the quotes'}
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
        {onCaretPlaced}
      />
    </div>
  {/each}
</div>

<style>
  .composer-draft-quotes {
    display: flex;
    flex-direction: column;
    flex-shrink: 0;
  }

  /* A gap and its quote, kept together for focus and lookup; no box of its own. */
  .composer-draft-entry {
    display: contents;
  }

  /* Gaps share the main input's column and type; the field draws the box.
     With a pointer, the placeholder names the gap for assistive tech only:
     the field shows one placeholder, in its last segment, while the whole
     draft is empty. */
  .composer-draft-quotes :global(.composer-gap-input) {
    padding: 4px 12px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
    flex-shrink: 0;
  }

  .composer-draft-quotes :global(.composer-gap-input.is-empty::before) {
    content: none;
  }

  /* The first gap starts the field, like the main input does without quotes. */
  .composer-draft-entry:first-child :global(.composer-gap-input) {
    padding-top: 7px;
  }

  /* With a pointer that hovers, an empty gap is a strip between blocks that
     widens to a line of text while the caret is in it. Fine pointers only: a
     touch leaves `:hover` stuck on and focus is a tap, so a row that changed
     size on either would shrink under the finger. */
  @media (hover: hover) and (pointer: fine) {
    .composer-draft-quotes :global(.composer-gap-input.is-empty:not(:focus)) {
      --hashtag-input-min-height: 6px;
      padding-block: 2px;
      line-height: 1;
      cursor: text;
    }

    .composer-draft-entry:first-child :global(.composer-gap-input.is-empty:not(:focus)) {
      padding-top: 4px;
    }
  }

  @media (hover: none), (pointer: coarse) {
    /* No hover or text cursor to find a gap by: each one is a finger-sized
       row (44px, border-box) so text between quotes has a place a tap can
       land, and an empty one shows its hint, faintly, so the row reads as a
       place to type rather than as space. */
    .composer-draft-quotes :global(.composer-gap-input) {
      --hashtag-input-min-height: 44px;
      padding: 12px;
      line-height: 1.5;
    }

    .composer-draft-quotes :global(.composer-gap-input.is-empty::before) {
      content: attr(data-placeholder);
      opacity: 0.6;
    }
  }
</style>
