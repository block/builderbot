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
    primaryLabel?: string;
  }

  let {
    viewModel,
    copyable = false,
    includePrimary = true,
    includeRaw = true,
    includeStatus = true,
    includeStreams = true,
    primaryLabel = 'Output',
  }: Props = $props();

  let copiedKey = $state<string | null>(null);
  let blocks = $derived(
    outputBlocks(viewModel, { includePrimary, includeRaw, includeStreams, primaryLabel })
  );

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
    options: Pick<Props, 'includePrimary' | 'includeRaw' | 'includeStreams' | 'primaryLabel'>
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
          label: section.source === 'primary' ? (options.primaryLabel ?? 'Output') : section.label,
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

{#each blocks as block}
  <section class="tool-output-section">
    <div class="tool-output-header">
      <div class="tool-panel-label">{block.label}</div>
      {#if copyable}
        <button
          type="button"
          class="tool-copy-button"
          title={copiedKey === block.key ? 'Copied' : `Copy ${block.label.toLowerCase()}`}
          aria-label={copiedKey === block.key ? 'Copied' : `Copy ${block.label.toLowerCase()}`}
          onclick={() => copyOutput(block.text, block.key)}
        >
          {#if copiedKey === block.key}
            <Check size={12} />
          {:else}
            <Copy size={12} />
          {/if}
        </button>
      {/if}
    </div>
    <pre
      class="tool-code-output"
      class:tool-output-danger={block.tone === 'danger'}
      class:tool-output-cancelled={block.tone === 'cancelled'}>{block.text}</pre>
  </section>
{/each}

{#if viewModel.output.emptyLabel}
  <div class="tool-empty-row">{viewModel.output.emptyLabel}</div>
{/if}

{#if includeStatus}
  <div
    class="tool-code-status"
    class:status-danger={viewModel.statusTone === 'danger'}
    class:status-cancelled={viewModel.statusTone === 'cancelled'}
  >
    {#if viewModel.statusTone === 'success'}
      <Check size={11} />
    {/if}
    {viewModel.statusLabel}
  </div>
{/if}
