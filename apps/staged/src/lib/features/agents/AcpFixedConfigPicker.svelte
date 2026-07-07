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

  let triggerParts = $derived(
    [
      providerLabel ?? providerId ?? 'Agent',
      selectorValueLabel(modelSelector, selectedModelValue),
      selectorValueLabel(effortSelector, selectedEffortValue),
    ].filter((part): part is string => !!part)
  );
  let shouldRender = $derived(
    !!providerId && (!!modelSelector || !!effortSelector || loading || !!error)
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
        'inline-flex h-9 max-w-[220px] shrink-0 items-center gap-1 rounded-[10px] px-2 text-xs text-muted-foreground transition-colors hover:bg-[var(--bg-hover)] focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40',
        triggerClass
      )}
      {disabled}
      title={disabled
        ? 'Configuration changes are available after this turn'
        : 'Select model and effort'}
    >
      <AgentIcon id={providerId ?? ''} size={14} />
      <span class="min-w-0 truncate whitespace-nowrap">{triggerParts.join(' · ')}</span>
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
      <div class="picker-column-grid">
        <div class="picker-column">
          <DropdownMenu.Label class="picker-section-label">Agent</DropdownMenu.Label>
          <DropdownMenu.Item disabled>
            <span class="inline-flex min-w-0 items-center gap-1.5">
              <AgentIcon id={providerId ?? ''} size={12} />
              <span class="truncate">{providerLabel ?? providerId ?? 'Agent'}</span>
            </span>
          </DropdownMenu.Item>
        </div>

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
      </div>

      {#if loading && !modelSelector && !effortSelector}
        <DropdownMenu.Separator />
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">
            <Spinner size={12} />
            Loading options…
          </span>
        </DropdownMenu.Item>
      {:else if error && !modelSelector && !effortSelector}
        <DropdownMenu.Separator />
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
