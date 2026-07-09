<script lang="ts">
  import type { ToolCallViewModel } from '../toolCallViewModel';
  import InlineToolDiff from './InlineToolDiff.svelte';
  import OutputSections from './OutputSections.svelte';

  interface Props {
    viewModel: ToolCallViewModel;
  }

  let { viewModel }: Props = $props();
  // The diff header already names the file; only surface path metadata it doesn't cover.
  let showPath = $derived(
    !!viewModel.metadata.targetPath &&
      !viewModel.metadata.diffs.some((diff) => diff.path === viewModel.metadata.targetPath)
  );
  let locations = $derived(
    viewModel.metadata.locations.filter(
      (location) =>
        location.display !== viewModel.metadata.targetPath &&
        !viewModel.metadata.diffs.some((diff) => diff.path === location.display)
    )
  );
</script>

<div class="tool-detail-stack">
  {#if showPath}
    <div class="tool-primary-row">
      <span class="tool-field-label">Path</span>
      <span class="tool-field-value">{viewModel.metadata.targetPath}</span>
    </div>
  {/if}

  {#if locations.length > 0}
    <div class="tool-meta-row">
      {#each locations as location}
        <span class="tool-chip">{location.display}</span>
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
