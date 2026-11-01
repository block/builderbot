<script lang="ts">
  import type { ToolCallViewModel } from '../toolCallViewModel';
  import OutputSections from './OutputSections.svelte';

  interface Props {
    viewModel: ToolCallViewModel;
  }

  let { viewModel }: Props = $props();

  /** Characters of the body shown before the reader asks for the rest. */
  const PREVIEW_LIMIT = 400;

  let note = $derived(viewModel.metadata.note);
  let body = $derived(note?.body.trim() ?? '');
  let truncated = $derived(body.length > PREVIEW_LIMIT);
  let showFullBody = $state(false);
  let visibleBody = $derived(
    truncated && !showFullBody ? `${body.slice(0, PREVIEW_LIMIT).trimEnd()}…` : body
  );
  let hasSteps = $derived(!!(note?.suggestedNextCommitStep || note?.suggestedNextNoteStep));
</script>

<div class="tool-detail-stack">
  {#if note}
    <div class="tool-command-panel tool-note-panel">
      {#if note.title}
        <div class="tool-note-title">{note.title}</div>
      {/if}
      {#if body}
        <pre class="tool-code-output tool-note-body">{visibleBody}</pre>
        {#if truncated}
          <button
            type="button"
            class="tool-note-toggle"
            aria-expanded={showFullBody}
            onclick={() => (showFullBody = !showFullBody)}
          >
            {showFullBody ? 'Show less' : 'Show full note'}
          </button>
        {/if}
      {:else}
        <div class="tool-empty-row">No note body</div>
      {/if}
      {#if hasSteps}
        <div class="tool-field-list">
          {#if note.suggestedNextCommitStep}
            <span class="tool-field-label">Next commit</span>
            <span class="tool-field-value">{note.suggestedNextCommitStep}</span>
          {/if}
          {#if note.suggestedNextNoteStep}
            <span class="tool-field-label">Next note</span>
            <span class="tool-field-value">{note.suggestedNextNoteStep}</span>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  <OutputSections {viewModel} />
</div>

<style>
  .tool-note-title {
    color: var(--text-primary);
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .tool-note-body {
    max-height: 160px;
  }

  .tool-note-toggle {
    align-self: flex-start;
    border: 0;
    padding: 0;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }

  .tool-note-toggle:hover,
  .tool-note-toggle:focus-visible {
    color: var(--text-primary);
    outline: none;
  }
</style>
