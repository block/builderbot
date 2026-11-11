<!--
  ComposerQuoteBlock.svelte — One quote block in the reply composer: the
  quoted note text with its heading path, an X to take it out of the draft,
  and the user's reply to it, editable in place.

  Pressing the X doesn't take focus: `onRemove` moves focus on from the input
  it was in, and WebKit would otherwise have blurred that input to the page
  by the time the click lands.
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import { Button } from '$lib/components/ui/button';
  import HashtagInput from './HashtagInput.svelte';
  import QuoteExcerpt from './QuoteExcerpt.svelte';
  import type { DraftQuote } from './composerDraft';
  import type { HashtagItem } from '../../types';

  interface Props {
    quote: DraftQuote;
    hashtagItems?: HashtagItem[];
    onkeydown?: (e: KeyboardEvent) => void;
    oninput?: (e: Event) => void;
    /** An IME or dictation composition is open in the reply. */
    composing?: boolean;
    oncompositionend?: () => void;
    onRemove: () => void;
  }

  let {
    quote = $bindable(),
    hashtagItems = [],
    onkeydown,
    oninput,
    composing = $bindable(false),
    oncompositionend,
    onRemove,
  }: Props = $props();
</script>

<div class="composer-quote" data-quote-id={quote.id}>
  <div class="composer-quote-excerpt">
    <QuoteExcerpt quote={quote.quote} headings={quote.headings} clampLines={3} />
  </div>
  <Button
    variant="ghost"
    size="icon-xs"
    class="composer-quote-remove touch-hit size-5 rounded text-[var(--text-faint)] hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-3"
    title="Remove quote"
    aria-label="Remove quote"
    onmousedown={(event: MouseEvent) => event.preventDefault()}
    onclick={onRemove}
  >
    <X size={12} />
  </Button>
  <HashtagInput
    bind:value={quote.reply}
    class="composer-quote-reply"
    placeholder="Reply to this quote…"
    rows={1}
    items={hashtagItems}
    bind:composing
    {oncompositionend}
    {onkeydown}
    {oninput}
  />
</div>

<style>
  .composer-quote {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    background: var(--bg-primary);
    transition: border-color 0.1s;
  }

  .composer-quote:focus-within {
    border-color: var(--border-emphasis);
  }

  /* Clear of the remove button in the corner. */
  .composer-quote-excerpt {
    padding-right: 22px;
  }

  .composer-quote :global(.composer-quote-remove) {
    position: absolute;
    top: 5px;
    right: 5px;
  }

  .composer-quote :global(.composer-quote-reply) {
    padding: 0;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
    min-height: 1.5em;
  }
</style>
