<!--
  QuoteExcerpt.svelte — A quoted passage of a note: the heading path it sits
  under as a breadcrumb, then the text behind a note-coloured rule.

  Shared by the note's selection popover, the composer's quote blocks and the
  transcript's sent quotes, so a quote reads the same from selection to send.
  `clampLines` caps the text's height (0: show it all); the full text is
  always in its tooltip.
-->
<script lang="ts">
  import { HEADING_PATH_SEPARATOR } from './composerDraft';

  interface Props {
    quote: string;
    headings: string[];
    /** Lines of quote text to show before cutting off; 0 shows everything. */
    clampLines?: number;
  }

  let { quote, headings, clampLines = 0 }: Props = $props();

  let headingPath = $derived(headings.join(HEADING_PATH_SEPARATOR));
</script>

<div class="quote-excerpt">
  {#if headings.length > 0}
    <div class="quote-path" title={headingPath}>
      <span class="sr-only">From the note section {headingPath}</span>
      {#each headings as heading, index (index)}
        {#if index > 0}
          <span class="quote-path-separator" aria-hidden="true">›</span>
        {/if}
        <span class="quote-path-segment" aria-hidden="true">{heading}</span>
      {/each}
    </div>
  {/if}
  <blockquote
    class="quote-text"
    class:clamped={clampLines > 0}
    style:--quote-clamp-lines={clampLines > 0 ? clampLines : undefined}
    title={clampLines > 0 ? quote : undefined}
  >
    {quote}
  </blockquote>
</div>

<style>
  .quote-excerpt {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  .quote-path {
    display: flex;
    align-items: baseline;
    gap: 4px;
    min-width: 0;
    overflow: hidden;
    font-size: var(--size-xs);
    line-height: 1.4;
    color: var(--text-muted);
    white-space: nowrap;
  }

  /* Ancestors give way first, so the section the text is in stays legible. */
  .quote-path-segment {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    flex-shrink: 1;
  }

  .quote-path-segment:last-child {
    flex-shrink: 0;
    max-width: 100%;
    color: var(--text-secondary);
    font-weight: 500;
  }

  .quote-path-separator {
    flex-shrink: 0;
  }

  .quote-text {
    margin: 0;
    padding: 1px 0 1px 9px;
    border-left: 2px solid var(--note-color);
    color: var(--text-secondary);
    font-size: var(--size-sm);
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .quote-text.clamped {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: var(--quote-clamp-lines);
    line-clamp: var(--quote-clamp-lines);
    overflow: hidden;
  }
</style>
