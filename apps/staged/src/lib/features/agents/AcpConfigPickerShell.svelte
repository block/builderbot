<!-- Shared trigger and dropdown layout for ACP config pickers. -->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import type { Snippet } from 'svelte';
  import { fade, fly } from 'svelte/transition';
  import AgentIcon from './AgentIcon.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { cn } from '$lib/components/utils';
  import { handleAcpPickerGridKeydown, handleAcpPickerOpenAutoFocus } from './acpPickerKeyboard';

  interface TriggerPart {
    id: string;
    label: string;
  }

  interface Props {
    providerId: string | null;
    triggerLabel: string;
    triggerParts?: TriggerPart[];
    triggerTitle: string;
    disabled?: boolean;
    dropUp?: boolean;
    triggerClass?: string;
    loading?: boolean;
    canOpen?: boolean;
    hasColumns?: boolean;
    /**
     * horizontal: all parts inline in the same style, separated by "·".
     * vertical: first part is the title; later parts stack below it in
     * smaller text.
     */
    layout?: 'horizontal' | 'vertical';
    children?: Snippet;
    footer?: Snippet;
  }

  let {
    providerId,
    triggerLabel,
    triggerParts = [],
    triggerTitle,
    disabled = false,
    dropUp = false,
    triggerClass,
    loading = false,
    canOpen = true,
    hasColumns = true,
    layout = 'horizontal',
    children,
    footer,
  }: Props = $props();

  let open = $state(false);
  let contentEl = $state<HTMLElement | null>(null);
  let labelSizerEl = $state<HTMLElement | null>(null);
  let labelWidth = $state<number | null>(null);
  let renderedTriggerParts = $derived(
    triggerParts.length > 0 ? triggerParts : [{ id: 'label', label: triggerLabel }]
  );

  // The hidden sizer always lays out at the natural width of the current
  // parts, so the visible label can transition to it when values change.
  $effect(() => {
    const sizer = labelSizerEl;
    if (!sizer) {
      labelWidth = null;
      return;
    }
    const observer = new ResizeObserver(() => {
      // Round up so sub-pixel clipping never triggers part ellipsis.
      labelWidth = Math.ceil(sizer.getBoundingClientRect().width);
    });
    observer.observe(sizer);
    return () => observer.disconnect();
  });

  function handlePickerKeydown(event: KeyboardEvent) {
    handleAcpPickerGridKeydown(event, contentEl, {
      onDismiss: () => {
        open = false;
      },
    });
  }
</script>

{#snippet labelContent()}
  <span
    class="selector-label"
    class:selector-label-vertical={layout === 'vertical'}
    aria-label={triggerLabel}
    style:width={labelWidth === null ? undefined : `${labelWidth}px`}
  >
    {#each renderedTriggerParts as part, index (part.id)}
      {#if layout === 'horizontal' && index > 0}
        <span class="trigger-separator" aria-hidden="true">·</span>
      {/if}
      <span class="trigger-part" class:trigger-part-subtitle={layout === 'vertical' && index > 0}>
        {#key part.label}
          <span
            class="trigger-part-value"
            in:fly={{ x: 4, duration: 120 }}
            out:fade={{ duration: 80 }}
          >
            {part.label}
          </span>
        {/key}
      </span>
    {/each}
    <span class="trigger-sizer" aria-hidden="true" bind:this={labelSizerEl}>
      {#each renderedTriggerParts as part, index (part.id)}
        {#if layout === 'horizontal' && index > 0}
          <span class="trigger-separator">·</span>
        {/if}
        <span class:trigger-part-subtitle={layout === 'vertical' && index > 0}>{part.label}</span>
      {/each}
    </span>
  </span>
{/snippet}

{#if canOpen}
  <DropdownMenu.Root bind:open>
    <DropdownMenu.Trigger
      class={cn(
        'selector-btn inline-flex min-w-0 items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-[var(--bg-hover)] focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40',
        triggerClass
      )}
      {disabled}
      title={triggerTitle}
    >
      <AgentIcon id={providerId ?? ''} size={12} />
      {@render labelContent()}
      <span class="trigger-caret" aria-hidden="true">
        {#if loading}
          <span class="trigger-caret-icon"><Spinner size={12} /></span>
        {:else}
          <span class="trigger-caret-icon"><ChevronDown size={12} /></span>
        {/if}
      </span>
    </DropdownMenu.Trigger>
    <DropdownMenu.Content
      bind:ref={contentEl}
      align="start"
      side={dropUp ? 'top' : 'bottom'}
      sideOffset={4}
      class="max-h-[min(360px,calc(100vh-48px))] max-w-[calc(100vw-16px)]"
      onOpenAutoFocus={(event) => handleAcpPickerOpenAutoFocus(event, contentEl)}
      onkeydowncapture={handlePickerKeydown}
    >
      {#if hasColumns}
        <div class="picker-column-grid">
          {@render children?.()}
        </div>
      {/if}

      {@render footer?.()}
    </DropdownMenu.Content>
  </DropdownMenu.Root>
{:else}
  <button
    type="button"
    class={cn(
      'selector-btn inline-flex min-w-0 items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground disabled:cursor-not-allowed disabled:opacity-40',
      triggerClass
    )}
    {disabled}
    title={triggerTitle}
  >
    <AgentIcon id={providerId ?? ''} size={12} />
    {@render labelContent()}
  </button>
{/if}

<style>
  /* Keep the trigger on its own compositing layer. The app's 13px rem base
     gives the trigger fractional-pixel geometry (e.g. gap-1.5 = 4.875px), and
     partial repaints while typing in a neighboring composer re-rasterize the
     surrounding layer, flipping the anti-aliasing of edges that sit between
     device pixels — a subtle shimmer of the icon/border. Isolated, the
     trigger only re-rasterizes when its own content changes. */
  :global(.selector-btn) {
    transform: translateZ(0);
  }

  .selector-label {
    display: inline-flex;
    position: relative;
    min-width: 0;
    align-items: center;
    overflow: hidden;
    white-space: nowrap;
    transition: width 150ms ease;
  }

  .selector-label-vertical {
    flex-direction: column;
    align-items: flex-start;
  }

  .selector-label-vertical .trigger-part {
    max-width: 100%;
    line-height: 1.25;
  }

  .trigger-part-subtitle {
    font-size: 0.85em;
  }

  .trigger-sizer {
    display: inline-flex;
    position: absolute;
    top: 0;
    left: 0;
    width: max-content;
    visibility: hidden;
    pointer-events: none;
    white-space: nowrap;
  }

  .selector-label-vertical .trigger-sizer {
    flex-direction: column;
    align-items: flex-start;
  }

  /* Fixed-size slot so swapping the spinner for the chevron never changes
     how much horizontal space the trailing icon takes (the icons themselves
     could otherwise shrink unevenly when the trigger is width-constrained). */
  .trigger-caret {
    position: relative;
    flex: none;
    width: 12px;
    height: 12px;
  }

  .trigger-caret-icon {
    display: inline-flex;
    position: absolute;
    align-items: center;
    justify-content: center;
    inset: 0;
  }

  .trigger-part {
    display: inline-grid;
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
  }

  .trigger-part-value {
    grid-area: 1 / 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .trigger-separator {
    flex: 0 0 auto;
    padding: 0 0.25rem;
  }

  .picker-column-grid {
    display: inline-grid;
    grid-auto-columns: max-content;
    grid-auto-flow: column;
    gap: 4px;
    max-width: calc(100vw - 16px);
    min-width: 0;
  }

  :global(.picker-column) {
    max-width: min(260px, calc(100vw - 24px));
    min-width: 0;
  }

  :global(.picker-column [data-slot='dropdown-menu-item']),
  :global(.picker-column [data-slot='dropdown-menu-radio-item']) {
    max-width: 100%;
    min-width: 0;
  }

  :global(.picker-column + .picker-column) {
    border-left: 1px solid var(--border-muted);
    padding-left: 4px;
  }

  :global(.picker-section-label) {
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  :global(.picker-status-row) {
    display: inline-flex;
    min-width: 0;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  :global(.picker-status-text) {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @media (max-width: 560px) {
    .picker-column-grid {
      grid-template-columns: 1fr;
    }

    :global(.picker-column + .picker-column) {
      border-left: 0;
      border-top: 1px solid var(--border-muted);
      padding-left: 0;
      padding-top: 4px;
    }
  }
</style>
