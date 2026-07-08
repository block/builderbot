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
    children,
    footer,
  }: Props = $props();

  let open = $state(false);
  let contentEl = $state<HTMLElement | null>(null);
  let renderedTriggerParts = $derived(
    triggerParts.length > 0 ? triggerParts : [{ id: 'label', label: triggerLabel }]
  );

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
      <span class="selector-label" aria-label={triggerLabel}>
        {#each renderedTriggerParts as part, index (part.id)}
          {#if index > 0}
            <span class="trigger-separator" aria-hidden="true">·</span>
          {/if}
          <span class="trigger-part">
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
      </span>
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
    <span class="selector-label" aria-label={triggerLabel}>
      {#each renderedTriggerParts as part, index (part.id)}
        {#if index > 0}
          <span class="trigger-separator" aria-hidden="true">·</span>
        {/if}
        <span class="trigger-part">
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
    </span>
  </button>
{/if}

<style>
  .selector-label {
    display: inline-flex;
    min-width: 0;
    align-items: center;
    overflow: hidden;
    white-space: nowrap;
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
