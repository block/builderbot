<!-- Compact ACP model/effort picker for an existing fixed-provider session. -->
<script lang="ts">
  import AcpConfigPickerSection from './AcpConfigPickerSection.svelte';
  import AcpConfigPickerShell from './AcpConfigPickerShell.svelte';
  import type { AcpConfigSelector } from '../../api/commands';
  import Spinner from '../../shared/Spinner.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';

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

  let effortColumnWidth = $state(0);
  let retainedEffortColumnWidth = $state<number | null>(null);
  let retainedEffortTriggerLabel = $state<string | null>(null);
  let retainedProviderId = $state<string | null>(null);

  let loadingEffortOptions = $derived(loading && !!modelSelector && !effortSelector);
  let modelTriggerLabel = $derived(selectorValueLabel(modelSelector, selectedModelValue));
  let effortTriggerLabel = $derived(selectorValueLabel(effortSelector, selectedEffortValue));
  let displayedEffortTriggerLabel = $derived(
    effortTriggerLabel ?? (loadingEffortOptions ? retainedEffortTriggerLabel : null)
  );
  // The provider is identified by the trigger icon; the label only carries the
  // model/effort values, falling back to the provider name before discovery.
  let triggerParts = $derived(
    [
      modelTriggerLabel ? { id: 'model', label: modelTriggerLabel } : null,
      displayedEffortTriggerLabel ? { id: 'effort', label: displayedEffortTriggerLabel } : null,
    ].filter((part): part is { id: string; label: string } => !!part)
  );
  let triggerLabel = $derived(
    triggerParts.length > 0
      ? triggerParts.map((part) => part.label).join(' · ')
      : (providerLabel ?? providerId ?? 'Agent')
  );
  let renderedTriggerParts = $derived(
    triggerParts.length > 0
      ? triggerParts
      : [{ id: 'provider', label: providerLabel ?? providerId ?? 'Agent' }]
  );
  let shouldRender = $derived(
    !!providerId && (!!modelSelector || !!effortSelector || loading || !!error)
  );
  let hasPickerColumns = $derived(
    !!modelSelector || !!effortSelector || loadingEffortOptions || (loading && !modelSelector)
  );
  let effortColumnStyle = $derived(
    loadingEffortOptions && retainedEffortColumnWidth
      ? `width: ${retainedEffortColumnWidth}px;`
      : undefined
  );

  $effect(() => {
    const nextProviderId = providerId ?? null;
    if (nextProviderId === retainedProviderId) return;

    retainedProviderId = nextProviderId;
    retainedEffortTriggerLabel = null;
    retainedEffortColumnWidth = null;
    effortColumnWidth = 0;
  });

  $effect(() => {
    if (!loading && effortTriggerLabel) {
      retainedEffortTriggerLabel = effortTriggerLabel;
    }
  });

  $effect(() => {
    if (!loading && effortSelector && effortColumnWidth > 0) {
      retainedEffortColumnWidth = Math.ceil(effortColumnWidth);
    }
  });

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
  <AcpConfigPickerShell
    {providerId}
    {triggerLabel}
    triggerParts={renderedTriggerParts}
    triggerTitle={disabled
      ? 'Configuration changes are available after this turn'
      : `Select model and effort (${providerLabel ?? providerId ?? 'Agent'})`}
    {loading}
    {disabled}
    {dropUp}
    {triggerClass}
    hasColumns={hasPickerColumns}
  >
    {#if modelSelector}
      <div class="picker-column" data-picker-column="model">
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
      <div class="picker-column" data-picker-column="effort" bind:clientWidth={effortColumnWidth}>
        <AcpConfigPickerSection
          title={effortSelector.label || 'Effort'}
          selector={effortSelector}
          value={selectedEffortValue}
          {disabled}
          onValueChange={(value) => onEffortChange?.(value)}
        />
      </div>
    {:else if loadingEffortOptions}
      <div class="picker-column" data-picker-column="effort" style={effortColumnStyle}>
        <DropdownMenu.Label class="picker-section-label">Effort</DropdownMenu.Label>
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">
            <Spinner size={12} />
            <span class="picker-status-text">Loading options…</span>
          </span>
        </DropdownMenu.Item>
      </div>
    {/if}

    {#if loading && !loadingEffortOptions && (!modelSelector || !effortSelector)}
      <div class="picker-column" data-picker-column="status">
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">
            <Spinner size={12} />
            <span class="picker-status-text">Loading options…</span>
          </span>
        </DropdownMenu.Item>
      </div>
    {/if}

    {#snippet footer()}
      {#if error && !modelSelector && !effortSelector}
        {#if hasPickerColumns}
          <DropdownMenu.Separator />
        {/if}
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">Using provider defaults</span>
        </DropdownMenu.Item>
      {/if}
    {/snippet}
  </AcpConfigPickerShell>
{/if}
