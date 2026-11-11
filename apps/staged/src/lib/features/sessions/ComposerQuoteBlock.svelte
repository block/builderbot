<!--
  ComposerQuoteBlock.svelte — One quote block embedded in the reply
  composer's field: the quoted note text with its heading path, an X to take
  it out of the draft, and the user's reply to it, editable in place.

  The block is an inset in the field rather than a card of its own — a
  rounded tint of the field's surface with no border, so the free text around
  it and the reply inside it read as one message. A click on the block's own
  padding puts the caret at the end of its reply; the inputs and the X are
  left to themselves, and a drag that ends on the padding keeps the text it
  selected.

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
  import { focusAtEndSync } from '../../shared/focusAtEnd';

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

  let blockEl = $state<HTMLDivElement>();
  let replyEl = $state<HTMLElement | null>(null);

  /**
   * A press on the block's own padding or the excerpt's margin puts the
   * caret at the end of the reply. On mousedown with the default prevented,
   * as the field does for its surface, so the input that had focus isn't
   * blurred to the page in between. A press on the quote text, the reply or
   * the X means those, and a drag over the quote text is left to select it.
   */
  function handleMousedown(event: MouseEvent) {
    if (event.button !== 0 || event.shiftKey) return;
    const target = event.target;
    if (!(target instanceof HTMLElement)) return;
    if (target !== blockEl && !target.classList.contains('composer-quote-excerpt')) return;
    event.preventDefault();
    focusAtEndSync(replyEl);
  }
</script>

<!-- A pointer convenience: keyboard users reach the reply by Tab or the arrows. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="composer-quote"
  data-quote-id={quote.id}
  bind:this={blockEl}
  onmousedown={handleMousedown}
>
  <div class="composer-quote-excerpt">
    <QuoteExcerpt quote={quote.quote} headings={quote.headings} clampLines={3} />
  </div>
  <Button
    variant="ghost"
    size="icon-xs"
    class="composer-quote-remove touch-hit size-5 rounded text-[var(--text-faint)] hover:text-foreground [&_svg]:!size-3"
    title="Remove quote"
    aria-label="Remove quote"
    onmousedown={(event: MouseEvent) => event.preventDefault()}
    onclick={onRemove}
  >
    <X size={12} />
  </Button>
  <HashtagInput
    bind:textareaEl={replyEl}
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
    gap: 4px;
    margin: 0 8px;
    padding: 8px 10px 6px;
    border-radius: 8px;
    background: var(--bg-hover);
    cursor: text;
    flex-shrink: 0;
  }

  /* Clear of the remove button in the corner. */
  .composer-quote-excerpt {
    padding-right: 22px;
  }

  .composer-quote :global(.composer-quote-remove) {
    position: absolute;
    top: 5px;
    right: 5px;
    cursor: default;
  }

  /* The block is already the hover tint; the X needs a tint of its own. */
  .composer-quote :global(.composer-quote-remove:hover) {
    background: color-mix(in srgb, var(--text-primary) 10%, transparent);
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

  /* Finger-sized: the X is drawn at 32px (its `touch-hit` area makes up
     44px) with the excerpt kept clear of it, and the reply row is 44px tall
     (border-box: the 1.5em line plus 12px above and below, the min-height
     making up the half pixel) so a tap on it lands in the input rather than
     beside it. */
  @media (pointer: coarse) {
    .composer-quote {
      gap: 2px;
      padding: 10px 12px 0;
    }

    .composer-quote-excerpt {
      padding-right: 34px;
    }

    .composer-quote :global(.composer-quote-remove) {
      top: 4px;
      right: 4px;
      width: 32px;
      height: 32px;
    }

    .composer-quote :global(.composer-quote-remove svg) {
      width: 16px;
      height: 16px;
    }

    .composer-quote :global(.composer-quote-reply) {
      --hashtag-input-min-height: 44px;
      padding: 12px 0;
    }
  }
</style>
