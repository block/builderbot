<script lang="ts">
  import { slide } from 'svelte/transition';
  import type { Snippet } from 'svelte';
  import ExternalLink from '@lucide/svelte/icons/external-link';
  import { Button } from '$lib/components/ui/button';
  import type { ToolCallBody } from '../../../types';
  import type { RichToolItem } from '../acpTranscript';
  import type { DisplayRootInput } from '../pathDisplayRoots';
  import { loadToolCallBodies } from '../toolBodySource';
  import { deferredToolBodyIds, hydrateToolItem } from '../toolCallBodies';
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

  // The store ships the large bodies of settled tool rows as markers
  // (`toolCallBodies.ts`); they are fetched once the card is expanded and
  // swapped into the item before any detail renders. `bodies` accumulates
  // per card so a re-expand is instant; `toolBodySource` also caches across
  // cards and mounts.
  let bodies = $state<ReadonlyMap<number, ToolCallBody>>(new Map());
  let bodyError = $state<string | null>(null);
  let deferredIds = $derived(deferredToolBodyIds(item));
  let missingIds = $derived(deferredIds.filter((id) => !bodies.has(id)));
  let bodiesReady = $derived(missingIds.length === 0);
  let hydratedItem = $derived(bodiesReady ? hydrateToolItem(item, bodies) : item);

  let viewModel = $derived(buildToolCallViewModel(hydratedItem, displayRoots));
  let showInlineDiagram = $derived(item.pikchrRenderSource !== null && !!diagram);
  let showSessionButton = $derived(
    !!(item.isPikchrDiagramTool && item.innerSessionId && onOpenSession)
  );

  $effect(() => {
    if (!expanded || missingIds.length === 0 || bodyError !== null) return;
    const ids = missingIds;
    let active = true;
    loadToolCallBodies(ids).then(
      (loaded) => {
        if (active) bodies = new Map([...bodies, ...loaded]);
      },
      (error: unknown) => {
        if (active) bodyError = error instanceof Error ? error.message : String(error);
      }
    );
    return () => {
      active = false;
    };
  });

  function retryBodies() {
    bodyError = null;
  }
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
      {#if bodiesReady}
        <ToolCallDetails item={hydratedItem} {viewModel} {displayRoots} />
      {:else if bodyError !== null}
        <div class="tool-body-notice tool-body-notice-failed" role="alert">
          <span>Couldn't load the tool details.</span>
          <button type="button" class="tool-body-retry" onclick={retryBodies}>Retry</button>
        </div>
      {:else}
        <div class="tool-body-notice" role="status">Loading details…</div>
      {/if}
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

  /* Same footprint as the details block, so the card doesn't jump when the
     fetched body replaces the notice. */
  .tool-body-notice {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 4px;
    border-radius: 8px;
    padding: 12px 14px;
    background: color-mix(in srgb, var(--bg-chrome) 82%, var(--bg-primary));
    color: var(--text-faint);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: calc(var(--size-xs) * 0.9);
    line-height: 1.5;
  }

  .tool-body-notice-failed {
    color: var(--ui-danger);
  }

  .tool-body-retry {
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    padding: 2px 8px;
    background: transparent;
    color: var(--text-muted);
    font: inherit;
    cursor: pointer;
  }

  .tool-body-retry:hover,
  .tool-body-retry:focus-visible {
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
    color: var(--text-primary);
    outline: none;
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
