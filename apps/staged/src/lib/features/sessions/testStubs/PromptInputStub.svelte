<!--
  Test stub for HashtagInput: a bare contenteditable that mirrors the two-way
  `value` binding, so component tests can simulate typing without pulling in
  the hashtag editor's dependencies. Stands in for ChatComposer too, so it
  echoes the composer's `sendError` where tests can read it.
-->
<script lang="ts">
  let {
    value = $bindable(''),
    textareaEl = $bindable(null),
    sendError = null,
  }: {
    value?: string;
    textareaEl?: HTMLElement | null;
    sendError?: string | null;
  } = $props();
</script>

<div
  bind:this={textareaEl}
  role="textbox"
  aria-label="prompt"
  tabindex="0"
  contenteditable="true"
  data-testid="prompt-input"
  oninput={(e) => (value = e.currentTarget.textContent ?? '')}
></div>
{#if sendError}
  <p data-testid="composer-send-error">{sendError}</p>
{/if}
