<script lang="ts">
  import type { ToolCallViewModel } from '../toolCallViewModel';
  import OutputSections from './OutputSections.svelte';

  interface Props {
    viewModel: ToolCallViewModel;
  }

  let { viewModel }: Props = $props();
  let commandText = $derived((viewModel.metadata.command ?? viewModel.detail) || 'Command');
  // Status already shows in the header dot and the footer; only surface failure exits here.
  let failureExitCode = $derived(
    viewModel.output.exitCode !== null && viewModel.output.exitCode !== 0
      ? viewModel.output.exitCode
      : null
  );
</script>

<div class="tool-detail-stack">
  <div class="tool-command-panel">
    <div class="tool-command-line">
      <span class="tool-command-prefix">$</span>
      <span class="tool-command-text">{commandText}</span>
    </div>
    {#if viewModel.metadata.workingDirectory || failureExitCode !== null}
      <div class="tool-field-list">
        {#if viewModel.metadata.workingDirectory}
          <span class="tool-field-label">Directory</span>
          <span class="tool-field-value">{viewModel.metadata.workingDirectory}</span>
        {/if}
        {#if failureExitCode !== null}
          <span class="tool-field-label">Exit</span>
          <span class="tool-field-value">{failureExitCode}</span>
        {/if}
      </div>
    {/if}
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
