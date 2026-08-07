<script lang="ts">
  import Check from '@lucide/svelte/icons/check';
  import Copy from '@lucide/svelte/icons/copy';
  import type { ToolCallViewModel } from '../toolCallViewModel';

  type OutputTone = 'normal' | 'danger' | 'cancelled';

  interface OutputBlock {
    key: string;
    label: string;
    text: string;
    tone: OutputTone;
  }

  interface Props {
    viewModel: ToolCallViewModel;
    copyable?: boolean;
    includePrimary?: boolean;
    includeRaw?: boolean;
    includeStatus?: boolean;
    includeStreams?: boolean;
  }

  let {
    viewModel,
    copyable = false,
    includePrimary = true,
    includeRaw = true,
    includeStatus = true,
    includeStreams = true,
  }: Props = $props();

  let copiedKey = $state<string | null>(null);
  let blocks = $derived(outputBlocks(viewModel, { includePrimary, includeRaw, includeStreams }));

  async function copyOutput(text: string, key: string) {
    try {
      await navigator.clipboard.writeText(text);
      copiedKey = key;
      setTimeout(() => {
        if (copiedKey === key) copiedKey = null;
      }, 1500);
    } catch {
      // Clipboard writes can fail outside secure browser contexts.
    }
  }

  // The view model's sections already encode which outputs to show (error /
  // stdout / stderr precedence, raw JSON only as a fallback); this just
  // projects them through the renderer's include options.
  function outputBlocks(
    model: ToolCallViewModel,
    options: Pick<Props, 'includePrimary' | 'includeRaw' | 'includeStreams'>
  ): OutputBlock[] {
    const result: OutputBlock[] = [];
    const defaultTone = model.status === 'cancelled' ? 'cancelled' : 'normal';

    for (const section of model.sections) {
      if (section.kind === 'output') {
        if (!options.includeStreams && (section.source === 'stdout' || section.source === 'stderr'))
          continue;
        if (section.source === 'primary' && !options.includePrimary) continue;
        result.push({
          key: section.source,
          // The primary block is the tool's main result — a "Content"/"Output"
          // header just restates what the card already makes obvious, so it
          // renders bare. Stdout/stderr/error keep labels to tell them apart.
          label: section.source === 'primary' ? '' : section.label,
          text: section.text,
          tone: section.tone,
        });
      } else if (section.kind === 'raw_output' && options.includeRaw) {
        result.push({
          key: 'raw-output',
          label: section.label,
          text: section.text,
          tone: defaultTone,
        });
      }
    }

    return result;
  }
</script>

{#snippet copyButton(block: OutputBlock, copyLabel: string)}
  <button
    type="button"
    class="tool-copy-button"
    title={copiedKey === block.key ? 'Copied' : `Copy ${copyLabel.toLowerCase()}`}
    aria-label={copiedKey === block.key ? 'Copied' : `Copy ${copyLabel.toLowerCase()}`}
    onclick={() => copyOutput(block.text, block.key)}
  >
    {#if copiedKey === block.key}
      <Check size={12} />
    {:else}
      <Copy size={12} />
    {/if}
  </button>
{/snippet}

{#each blocks as block}
  {@const copyLabel = block.label || 'output'}
  <section class="tool-output-section">
    {#if block.label}
      <div class="tool-output-header">
        <div class="tool-panel-label">{block.label}</div>
        {#if copyable}
          {@render copyButton(block, copyLabel)}
        {/if}
      </div>
    {/if}
    <div class="tool-output-body">
      <!-- A label-less output gets no header row; its copy affordance tucks
           into the top-right corner so the "$" and the button don't each eat a
           full row. -->
      {#if copyable && !block.label}
        <div class="tool-output-body-actions">
          {@render copyButton(block, copyLabel)}
        </div>
      {/if}
      <pre
        class="tool-code-output"
        class:tool-output-danger={block.tone === 'danger'}
        class:tool-output-cancelled={block.tone === 'cancelled'}>{block.text}</pre>
    </div>
  </section>
{/each}

{#if viewModel.output.emptyLabel}
  <div class="tool-empty-row">{viewModel.output.emptyLabel}</div>
{/if}

<!-- Success and in-progress rows only echo the check and clock icons already
     in the card header, so the footer status shows only when it adds something
     (failed, cancelled, pending). -->
{#if includeStatus && viewModel.statusTone !== 'success' && viewModel.statusTone !== 'running'}
  <div
    class="tool-code-status"
    class:status-danger={viewModel.statusTone === 'danger'}
    class:status-cancelled={viewModel.statusTone === 'cancelled'}
  >
    {viewModel.statusLabel}
  </div>
{/if}
