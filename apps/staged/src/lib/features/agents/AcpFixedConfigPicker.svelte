<!-- Compact ACP model/effort picker for an existing fixed-provider session. -->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import AgentIcon from './AgentIcon.svelte';
  import AcpConfigPickerSection from './AcpConfigPickerSection.svelte';
  import type { AcpConfigSelector } from '../../api/commands';
  import Spinner from '../../shared/Spinner.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { cn } from '$lib/components/utils';

  interface Props {
    providerId: string | null;
    providerLabel?: string | null;
    modelSelector?: AcpConfigSelector | null;
    effortSelector?: AcpConfigSelector | null;
    selectedModelValue?: string | null;
    selectedEffortValue?: string | null;
    loading?: boolean;
    error?: string | null;
    disabled?: boolean;
    dropUp?: boolean;
    triggerClass?: string;
    onModelChange?: (value: string) => void;
    onEffortChange?: (value: string) => void;
  }

  let {
    providerId,
    providerLabel = null,
    modelSelector = null,
    effortSelector = null,
    selectedModelValue = null,
    selectedEffortValue = null,
    loading = false,
    error = null,
    disabled = false,
    dropUp = false,
    triggerClass,
    onModelChange,
    onEffortChange,
  }: Props = $props();

  // The provider is identified by the trigger icon; the label only carries the
  // model/effort values, falling back to the provider name before discovery.
  let triggerParts = $derived(
    [
      selectorValueLabel(modelSelector, selectedModelValue),
      selectorValueLabel(effortSelector, selectedEffortValue),
    ].filter((part): part is string => !!part)
  );
  let triggerLabel = $derived(
    triggerParts.length > 0 ? triggerParts.join(' · ') : (providerLabel ?? providerId ?? 'Agent')
  );
  let shouldRender = $derived(
    !!providerId && (!!modelSelector || !!effortSelector || loading || !!error)
  );
  let hasPickerColumns = $derived(
    !!modelSelector || !!effortSelector || (loading && !modelSelector && !effortSelector)
  );

  function selectorValueLabel(
    selector: AcpConfigSelector | null,
    valueId: string | null
  ): string | null {
    if (!selector) return null;
    if (selector.options.length === 0) return 'Default';
    const option =
      selector.options.find((candidate) => candidate.valueId === valueId) ??
      selector.options.find((candidate) => candidate.valueId === selector.currentValueId);
    return option?.label ?? 'Default';
  }
</script>

{#if shouldRender}
  <DropdownMenu.Root>
    <DropdownMenu.Trigger
      class={cn(
        'selector-btn inline-flex min-w-0 items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-[var(--bg-hover)] focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40',
        triggerClass
      )}
      {disabled}
      title={disabled
        ? 'Configuration changes are available after this turn'
        : `Select model and effort (${providerLabel ?? providerId ?? 'Agent'})`}
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
      align="start"
      side={dropUp ? 'top' : 'bottom'}
      sideOffset={4}
      class="max-h-[min(360px,calc(100vh-48px))] max-w-[calc(100vw-16px)]"
    >
      {#if hasPickerColumns}
        <div class="picker-column-grid">
          {#if modelSelector}
            <div class="picker-column">
              <AcpConfigPickerSection
                title={modelSelector.label || 'Model'}
                selector={modelSelector}
                value={selectedModelValue}
                {disabled}
                onValueChange={(value) => onModelChange?.(value)}
              />
            </div>
          {/if}

          {#if effortSelector}
            <div class="picker-column">
              <AcpConfigPickerSection
                title={effortSelector.label || 'Effort'}
                selector={effortSelector}
                value={selectedEffortValue}
                {disabled}
                onValueChange={(value) => onEffortChange?.(value)}
              />
            </div>
          {/if}

          {#if loading && !modelSelector && !effortSelector}
            <div class="picker-column">
              <DropdownMenu.Item disabled>
                <span class="picker-status-row">
                  <Spinner size={12} />
                  Loading options…
                </span>
              </DropdownMenu.Item>
            </div>
          {/if}
        </div>
      {/if}

      {#if error && !modelSelector && !effortSelector}
        {#if hasPickerColumns}
          <DropdownMenu.Separator />
        {/if}
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">Using provider defaults</span>
        </DropdownMenu.Item>
      {/if}
    </DropdownMenu.Content>
  </DropdownMenu.Root>
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

  .picker-column {
    max-width: min(260px, calc(100vw - 24px));
    min-width: 0;
  }

  .picker-column :global([data-slot='dropdown-menu-item']),
  .picker-column :global([data-slot='dropdown-menu-radio-item']) {
    max-width: 100%;
    min-width: 0;
  }

  .picker-column + .picker-column {
    border-left: 1px solid var(--border-muted);
    padding-left: 4px;
  }

  :global(.picker-section-label) {
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .picker-status-row {
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

    .picker-column + .picker-column {
      border-left: 0;
      border-top: 1px solid var(--border-muted);
      padding-left: 0;
      padding-top: 4px;
    }
  }
</style>
