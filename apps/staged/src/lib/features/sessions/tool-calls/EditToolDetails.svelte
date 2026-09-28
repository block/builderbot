<script lang="ts">
  import { summarizeToolCallLocations, type ToolCallViewModel } from '../toolCallViewModel';
  import InlineToolDiff from './InlineToolDiff.svelte';
  import OutputSections from './OutputSections.svelte';

  interface Props {
    viewModel: ToolCallViewModel;
  }

  let { viewModel }: Props = $props();
  // The tool and diff headers name the files; chips retain location details.
  let locationSummary = $derived(
    summarizeToolCallLocations(viewModel.metadata.locations, null, [
      viewModel.metadata.targetPath,
      ...viewModel.metadata.diffs.map((diff) => diff.path),
    ])
  );
</script>

<div class="tool-detail-stack">
  {#if locationSummary.chips.length > 0}
    <div class="tool-meta-row">
      {#each locationSummary.chips as chip}
        <span class="tool-chip">{chip}</span>
      {/each}
    </div>
  {/if}

  {#if viewModel.metadata.diffs.length > 0}
    {#each viewModel.metadata.diffs as diff}
      <InlineToolDiff {diff} />
    {/each}
  {:else if viewModel.metadata.inputText}
    <section>
      <div class="tool-panel-label">Input</div>
      <pre class="tool-code-output">{viewModel.metadata.inputText}</pre>
    </section>
  {/if}

  <OutputSections {viewModel} />
</div>
