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
  <AcpConfigPickerShell
    {providerId}
    {triggerLabel}
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
      <div class="picker-column" data-picker-column="effort">
        <AcpConfigPickerSection
          title={effortSelector.label || 'Effort'}
          selector={effortSelector}
          value={selectedEffortValue}
          {disabled}
          onValueChange={(value) => onEffortChange?.(value)}
        />
      </div>
    {/if}

    {#if loading && (!modelSelector || !effortSelector)}
      <div class="picker-column" data-picker-column="status">
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">
            <Spinner size={12} />
            Loading options…
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
