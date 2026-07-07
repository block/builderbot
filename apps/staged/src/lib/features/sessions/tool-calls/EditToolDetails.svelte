<script lang="ts">
  import type { ToolCallViewModel } from '../toolCallViewModel';
  import InlineToolDiff from './InlineToolDiff.svelte';
  import OutputSections from './OutputSections.svelte';

  interface Props {
    viewModel: ToolCallViewModel;
  }

  let { viewModel }: Props = $props();
</script>

<div class="tool-detail-stack">
  {#if viewModel.metadata.targetPath}
    <div class="tool-primary-row">
      <span class="tool-field-label">Path</span>
      <span class="tool-field-value">{viewModel.metadata.targetPath}</span>
    </div>
  {/if}

  {#if viewModel.metadata.locations.length > 0}
    <div class="tool-meta-row">
      {#each viewModel.metadata.locations as location}
        <span class="tool-chip">{location.display}</span>
      {/each}
    </div>
  {/if}

  {#if viewModel.metadata.diffs.length > 0}
    {#each viewModel.metadata.diffs as diff}
      <section>
        <div class="tool-panel-label">{diff.path}</div>
        <InlineToolDiff {diff} />
      </section>
    {/each}
  {:else if viewModel.metadata.inputText}
    <section>
      <div class="tool-panel-label">Input</div>
      <pre class="tool-code-output">{viewModel.metadata.inputText}</pre>
    </section>
  {/if}

  <OutputSections {viewModel} />
</div>
