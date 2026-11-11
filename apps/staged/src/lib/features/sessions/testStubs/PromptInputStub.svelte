<!--
  Test stub for HashtagInput: a bare contenteditable that mirrors the two-way
  `value` binding, so component tests can simulate typing without pulling in
  the hashtag editor's dependencies. Stands in for ChatComposer too, so it
  echoes the composer's `sendError` where tests can read it, renders a
  button per new-session action when the pane offers them, mirrors what
  the pane put in the box (`data-value`) and whether the control it handed
  down is split (`data-send-split`), takes its bindable `quotes`, whose
  count it shows as `data-quote-count`, and offers a remove button per quote
  (`data-testid="remove-quote-<id>"`) that calls `onRemoveQuote` as the
  composer's X would, and a `focusQuoteReply` that places no caret.
-->
<script lang="ts">
  let {
    value = $bindable(''),
    quotes = $bindable([]),
    textareaEl = $bindable(null),
    sendError = null,
    sendControl,
    onStartNewSession,
    newSessionDisabled = false,
    onRemoveQuote,
  }: {
    value?: string;
    quotes?: Array<{ id: string }>;
    textareaEl?: HTMLElement | null;
    sendError?: string | null;
    sendControl?: { split: boolean };
    onStartNewSession?: (kind: 'note' | 'commit') => void;
    newSessionDisabled?: boolean;
    onRemoveQuote?: (id: string) => void;
  } = $props();

  /** The composer's caret placement, which the stub lays nothing out for. */
  export function focusQuoteReply(_id: string): boolean {
    return false;
  }
</script>

<div
  bind:this={textareaEl}
  role="textbox"
  aria-label="prompt"
  tabindex="0"
  contenteditable="true"
  data-testid="prompt-input"
  data-value={value}
  data-send-split={sendControl?.split}
  data-quote-count={quotes.length}
  oninput={(e) => (value = e.currentTarget.textContent ?? '')}
></div>
{#each quotes as quote (quote.id)}
  <button
    type="button"
    aria-label="Remove quote"
    data-testid="remove-quote-{quote.id}"
    onclick={() => onRemoveQuote?.(quote.id)}
  ></button>
{/each}
{#if sendError}
  <p data-testid="composer-send-error">{sendError}</p>
{/if}

{#if onStartNewSession}
  {#each ['note', 'commit'] as const as kind (kind)}
    <button
      type="button"
      data-testid="composer-new-{kind}"
      disabled={newSessionDisabled}
      onclick={() => onStartNewSession(kind)}>{kind}</button
    >
  {/each}
{/if}
