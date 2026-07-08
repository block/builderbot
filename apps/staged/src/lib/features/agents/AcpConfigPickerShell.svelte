<!-- Shared trigger and dropdown layout for ACP config pickers. -->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import type { Snippet } from 'svelte';
  import AgentIcon from './AgentIcon.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { cn } from '$lib/components/utils';
  import { handleAcpPickerGridKeydown, handleAcpPickerOpenAutoFocus } from './acpPickerKeyboard';

  interface Props {
    providerId: string | null;
    triggerLabel: string;
    triggerTitle: string;
    disabled?: boolean;
    dropUp?: boolean;
    triggerClass?: string;
    loading?: boolean;
    canOpen?: boolean;
    hasColumns?: boolean;
    children?: Snippet;
    footer?: Snippet;
  }

  let {
    providerId,
    triggerLabel,
    triggerTitle,
    disabled = false,
    dropUp = false,
    triggerClass,
    loading = false,
    canOpen = true,
    hasColumns = true,
    children,
    footer,
  }: Props = $props();

  let open = $state(false);
  let contentEl = $state<HTMLElement | null>(null);

  function handlePickerKeydown(event: KeyboardEvent) {
    handleAcpPickerGridKeydown(event, contentEl, {
      onDismiss: () => {
        open = false;
      },
    });
  }
</script>

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
      <span class="selector-label min-w-0 truncate whitespace-nowrap">{triggerLabel}</span>
      {#if loading}
        <Spinner size={12} />
      {:else}
        <ChevronDown size={12} />
      {/if}
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
    <span class="selector-label min-w-0 truncate whitespace-nowrap">{triggerLabel}</span>
  </button>
{/if}

<style>
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
