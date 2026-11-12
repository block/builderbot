<script lang="ts">
  import type { ToolCallViewModel } from '../toolCallViewModel';
  import OutputSections from './OutputSections.svelte';

  interface Props {
    viewModel: ToolCallViewModel;
  }

  let { viewModel }: Props = $props();

  // The call carries the title and the suggested next steps; the body is the
  // session's draft file, published into the stored note.
  let note = $derived(viewModel.metadata.note);
  let hasSteps = $derived(!!(note?.suggestedNextCommitStep || note?.suggestedNextNoteStep));
</script>

<div class="tool-detail-stack">
  {#if note && (note.title || hasSteps)}
    <div class="tool-command-panel tool-note-panel">
      {#if note.title}
        <div class="tool-note-title">{note.title}</div>
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
</style>
