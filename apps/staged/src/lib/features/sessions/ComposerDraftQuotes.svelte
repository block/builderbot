<!--
  ComposerDraftQuotes.svelte — The quote blocks of a reply draft, above the
  composer's main input, with the free text typed between them.

  The draft runs `text, quote, text, quote, …` and ends in the main input
  (see composerDraft.ts). Each gap before a quote is an input of its own that
  takes no room beyond the spacing between blocks: its placeholder shows on
  hover or focus, and it grows as text goes in. The list scrolls once a
  review outgrows the pane, and follows each quote added to its end.
-->
<script lang="ts">
  import HashtagInput from './HashtagInput.svelte';
  import ComposerQuoteBlock from './ComposerQuoteBlock.svelte';
  import type { DraftQuote } from './composerDraft';
  import type { HashtagItem } from '../../types';

  interface Props {
    quotes: DraftQuote[];
    hashtagItems?: HashtagItem[];
    onkeydown?: (e: KeyboardEvent) => void;
    oninput?: (e: Event) => void;
    onRemoveQuote: (id: string) => void;
  }

  let {
    quotes = $bindable(),
    hashtagItems = [],
    onkeydown,
    oninput,
    onRemoveQuote,
  }: Props = $props();

  let listEl = $state<HTMLDivElement>();
  let shownCount = 0;

  // A quote added at the end scrolls into view; edits and removals don't move
  // the list.
  $effect(() => {
    const count = quotes.length;
    const grew = count > shownCount;
    shownCount = count;
    if (!grew || !listEl) return;
    const last = listEl.querySelector<HTMLElement>('.composer-quote:last-of-type');
    last?.scrollIntoView({ block: 'nearest' });
  });
</script>

<div class="composer-draft-quotes" bind:this={listEl} role="group" aria-label="Quoted replies">
  {#each quotes as quote, index (quote.id)}
    <HashtagInput
      bind:value={quotes[index].before}
      class="composer-gap-input"
      placeholder={index === 0 ? 'Add a message…' : 'Add text between quotes…'}
      rows={1}
      items={hashtagItems}
      {onkeydown}
      {oninput}
    />
    <ComposerQuoteBlock
      bind:quote={quotes[index]}
      {hashtagItems}
      {onkeydown}
      {oninput}
      onRemove={() => onRemoveQuote(quote.id)}
    />
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
    /* No hover to reveal a gap: keep each one a tappable line. */
    .composer-draft-quotes :global(.composer-gap-input.is-empty) {
      --hashtag-input-min-height: 1.5em;
      padding-block: 3px;
      line-height: 1.5;
    }

    .composer-draft-quotes :global(.composer-gap-input.is-empty::before) {
      opacity: 0.6;
    }
  }
</style>
