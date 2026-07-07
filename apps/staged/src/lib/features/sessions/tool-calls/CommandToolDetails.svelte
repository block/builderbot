<script lang="ts">
  import type { ToolCallViewModel } from '../toolCallViewModel';
  import OutputSections from './OutputSections.svelte';

  interface Props {
    viewModel: ToolCallViewModel;
  }

  let { viewModel }: Props = $props();
  let commandText = $derived((viewModel.metadata.command ?? viewModel.detail) || 'Command');
</script>

<div class="tool-detail-stack">
  <div class="tool-command-panel">
    <div class="tool-command-line">
      <span class="tool-command-prefix">$</span>
      <span class="tool-command-text">{commandText}</span>
    </div>
    <div class="tool-field-list">
      {#if viewModel.metadata.workingDirectory}
        <span class="tool-field-label">Directory</span>
        <span class="tool-field-value">{viewModel.metadata.workingDirectory}</span>
      {/if}
      {#if viewModel.output.exitCode !== null}
        <span class="tool-field-label">Exit</span>
        <span class="tool-field-value">{viewModel.output.exitCode}</span>
      {/if}
      <span class="tool-field-label">Status</span>
      <span class="tool-field-value">{viewModel.statusLabel}</span>
    </div>
  </div>

  {#if viewModel.metadata.terminalRefs.length > 0}
    <div class="tool-meta-row">
      {#each viewModel.metadata.terminalRefs as terminalRef}
        <span class="tool-chip">{terminalRef}</span>
      {/each}
    </div>
  {/if}

  <OutputSections {viewModel} copyable />
</div>
