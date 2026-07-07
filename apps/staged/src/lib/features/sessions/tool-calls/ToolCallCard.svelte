<script lang="ts">
  import { slide } from 'svelte/transition';
  import type { Snippet } from 'svelte';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import { Button } from '$lib/components/ui/button';
  import type { RichToolItem } from '../acpTranscript';
  import type { DisplayRootInput } from '../pathDisplayRoots';
  import { buildToolCallViewModel } from '../toolCallViewModel';
  import ToolCallDetails from './ToolCallDetails.svelte';
  import ToolCallHeader from './ToolCallHeader.svelte';
  import ToolStatusDot from './ToolStatusDot.svelte';

  interface Props {
    item: RichToolItem;
    displayRoots?: DisplayRootInput;
    nested?: boolean;
    expanded: boolean;
    slideDuration?: number;
    onToggle: (key: string) => void;
    onOpenSession?: (sessionId: string) => void;
    /** Renders the inline diagram for a successful render_pikchr call. */
    diagram?: Snippet<[string]>;
  }

  let {
    item,
    displayRoots,
    nested = false,
    expanded,
    slideDuration = 150,
    onToggle,
    onOpenSession,
    diagram,
  }: Props = $props();

  let viewModel = $derived(buildToolCallViewModel(item, displayRoots));
  let showInlineDiagram = $derived(item.pikchrRenderSource !== null && !!diagram);
  let showSessionButton = $derived(
    !!(item.isPikchrDiagramTool && item.innerSessionId && onOpenSession)
  );
</script>

<div class="tool-card" class:tool-card-nested={nested}>
  {#if showInlineDiagram}
    {@render diagram?.(item.pikchrRenderSource!)}
  {:else if showSessionButton}
    <Button
      variant="outline"
      size="sm"
      class="tool-session-button {viewModel.statusTone === 'danger'
        ? 'tool-session-button-danger'
        : ''}"
      onclick={() => onOpenSession?.(item.innerSessionId!)}
    >
      <ToolStatusDot statusTone={viewModel.statusTone} />
      <span
        >{viewModel.statusTone === 'danger'
          ? 'Open failed diagram session'
          : 'Open diagram session'}</span
      >
      <ExternalLink size={12} />
    </Button>
  {:else}
    <ToolCallHeader
      verb={viewModel.verb}
      detail={viewModel.detail}
      statusTone={viewModel.statusTone}
      {expanded}
      expandable={viewModel.hasDetails}
      onToggle={() => onToggle(item.key)}
    />
  {/if}
  {#if expanded && viewModel.hasDetails && !showInlineDiagram && !showSessionButton}
    <div transition:slide={{ duration: slideDuration }}>
      <ToolCallDetails {item} {viewModel} />
    </div>
  {/if}
</div>

<style>
  .tool-card {
    overflow: hidden;
    min-width: 0;
  }

  .tool-card-nested {
    padding-left: 16px;
  }

  :global(.tool-session-button) {
    height: 26px;
    gap: 6px;
    border-color: var(--border-muted);
    padding: 0 8px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 500;
    box-shadow: none;
  }

  :global(.tool-session-button:hover) {
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  /* A failed generate_pikchr call keeps its session button (the child session
     records the failure); tint it so the error is visible without expanding. */
  :global(.tool-session-button-danger),
  :global(.tool-session-button-danger:hover) {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }
</style>
