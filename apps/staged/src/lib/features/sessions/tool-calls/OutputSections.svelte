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

  function outputBlocks(
    model: ToolCallViewModel,
    options: Pick<Props, 'includePrimary' | 'includeRaw' | 'includeStreams'>
  ): OutputBlock[] {
    const output = model.output;
    const result: OutputBlock[] = [];
    const defaultTone = model.status === 'cancelled' ? 'cancelled' : 'normal';

    if (output.errorText) {
      result.push({ key: 'error', label: 'Error', text: output.errorText, tone: 'danger' });
    }

    if (options.includeStreams && output.stdout) {
      result.push({ key: 'stdout', label: 'Stdout', text: output.stdout, tone: defaultTone });
    }

    if (options.includeStreams && output.stderr && output.stderr !== output.errorText) {
      result.push({
        key: 'stderr',
        label: 'Stderr',
        text: output.stderr,
        tone: model.status === 'failed' ? 'danger' : defaultTone,
      });
    }

    if (
      options.includePrimary &&
      output.primaryText &&
      output.primaryText !== output.errorText &&
      output.primaryText !== output.stdout &&
      output.primaryText !== output.stderr
    ) {
      result.push({ key: 'output', label: 'Output', text: output.primaryText, tone: defaultTone });
    }

    // Raw JSON is a fallback for output we could not render structurally,
    // never a companion to structured blocks.
    if (options.includeRaw && output.rawText && result.length === 0) {
      result.push({
        key: 'raw-output',
        label: 'Raw output',
        text: output.rawText,
        tone: defaultTone,
      });
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
