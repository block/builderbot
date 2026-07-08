<!--
  AcpConfigPicker.svelte — Compact provider/model/effort picker.

  Provider changes are persisted to the existing recent-agent preference so
  other launch paths keep using the selected provider. New-session launch paths
  can subscribe to the provider plus selected model/effort payload.
-->
<script lang="ts">
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import AgentIcon from './AgentIcon.svelte';
  import AcpConfigPickerSection from './AcpConfigPickerSection.svelte';
  import { agentState, REMOTE_AGENTS } from './agent.svelte';
  import { setAiAgent, getPreferredAgent } from '../settings/preferences.svelte';
  import {
    discoverAcpConfig,
    type AcpConfigDiscovery,
    type AcpConfigSelector,
  } from '../../api/commands';
  import Spinner from '../../shared/Spinner.svelte';
  import { cn } from '$lib/components/utils';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { buildAcpConfigSelection, type AcpConfigPickerSelection } from './acpConfigSelection';
  import { handleAcpPickerGridKeydown, handleAcpPickerOpenAutoFocus } from './acpPickerKeyboard';

  interface AgentOption {
    id: string;
    label: string;
  }

  interface Props {
    disabled?: boolean;
    remote?: boolean;
    dropUp?: boolean;
    triggerClass?: string;
    workingDir?: string | null;
    onSelectionChange?: (selection: AcpConfigPickerSelection) => void;
  }

  let {
    disabled = false,
    remote = false,
    dropUp = false,
    triggerClass,
    workingDir = null,
    onSelectionChange,
  }: Props = $props();

  let config = $state<AcpConfigDiscovery | null>(null);
  let configLoading = $state(false);
  let configError = $state<string | null>(null);
  let selectedModelValue = $state<string | null>(null);
  let selectedEffortValue = $state<string | null>(null);
  let modelSelectionExplicit = $state(false);
  let effortSelectionExplicit = $state(false);
  let modelSelectorKey = $state<string | null>(null);
  let effortSelectorKey = $state<string | null>(null);
  let open = $state(false);
  let contentEl = $state<HTMLElement | null>(null);
  let discoveryRun = 0;

  let agents = $derived<AgentOption[]>(remote ? REMOTE_AGENTS : agentState.providers);
  let selectedProviderId = $derived(getPreferredAgent(agents));
  let selectedProvider = $derived(agents.find((provider) => provider.id === selectedProviderId));
  let modelSelector = $derived(config?.model ?? null);
  let effortSelector = $derived(config?.effort ?? null);
  // The provider is identified by the trigger icon; the label only carries the
  // model/effort values, falling back to the provider name before discovery.
  let triggerParts = $derived(
    [
      selectorValueLabel(modelSelector, selectedModelValue),
      selectorValueLabel(effortSelector, selectedEffortValue),
    ].filter((part): part is string => !!part)
  );
  let triggerLabel = $derived(
    triggerParts.length > 0 ? triggerParts.join(' · ') : (selectedProvider?.label ?? 'Agent')
  );
  let canOpen = $derived(
    agents.length > 1 || !!modelSelector || !!effortSelector || configLoading || !!configError
  );
  let shouldRender = $derived((remote || agentState.loaded) && agents.length > 0);
  let pickerSelection = $derived({
    providerId: selectedProviderId,
    acpConfigSelection: buildAcpConfigSelection({
      model: {
        selector: modelSelector,
        valueId: selectedModelValue,
        explicit: modelSelectionExplicit,
      },
      effort: {
        selector: effortSelector,
        valueId: selectedEffortValue,
        explicit: effortSelectionExplicit,
      },
    }),
  } satisfies AcpConfigPickerSelection);

  $effect(() => {
    const providerId = selectedProviderId;
    const discoveryWorkingDir = workingDir ?? null;
    if (remote || !providerId) {
      config = null;
      configLoading = false;
      configError = null;
      return;
    }

    const run = ++discoveryRun;
    let cancelled = false;
    config = null;
    configLoading = true;
    configError = null;

    discoverAcpConfig(providerId, discoveryWorkingDir)
      .then(({ data, revalidating }) => {
        if (cancelled || run !== discoveryRun) return;
        config = data;
        configLoading = false;
        revalidating
          ?.then((fresh) => {
            if (!cancelled && run === discoveryRun) {
              config = fresh;
            }
          })
          .catch((error) => {
            if (!cancelled && run === discoveryRun) {
              console.error('Failed to revalidate ACP config:', error);
            }
          });
      })
      .catch((error) => {
        if (cancelled || run !== discoveryRun) return;
        console.error('Failed to discover ACP config:', error);
        config = null;
        configLoading = false;
        configError = error instanceof Error ? error.message : String(error);
      });

    return () => {
      cancelled = true;
    };
  });

  $effect(() => {
    const nextKey = selectorKey(modelSelector);
    if (nextKey !== modelSelectorKey) {
      selectedModelValue = defaultSelectorValue(modelSelector);
      modelSelectionExplicit = false;
      modelSelectorKey = nextKey;
    }
  });

  $effect(() => {
    const nextKey = selectorKey(effortSelector);
    if (nextKey !== effortSelectorKey) {
      selectedEffortValue = defaultSelectorValue(effortSelector);
      effortSelectionExplicit = false;
      effortSelectorKey = nextKey;
    }
  });

  $effect(() => {
    onSelectionChange?.(pickerSelection);
  });

  function handleProviderChange(providerId: string) {
    setAiAgent(providerId);
  }

  function handleModelChange(value: string) {
    selectedModelValue = value;
    modelSelectionExplicit = true;
  }

  function handleEffortChange(value: string) {
    selectedEffortValue = value;
    effortSelectionExplicit = true;
  }

  function handlePickerKeydown(event: KeyboardEvent) {
    handleAcpPickerGridKeydown(event, contentEl, {
      onDismiss: () => {
        open = false;
      },
    });
  }

  function selectorKey(selector: AcpConfigSelector | null): string | null {
    if (!selector) return null;
    const optionIds = selector.options.map((option) => option.valueId).join(',');
    return `${selector.configId}:${selector.currentValueId}:${optionIds}`;
  }

  function defaultSelectorValue(selector: AcpConfigSelector | null): string | null {
    if (!selector || selector.options.length === 0) return null;
    if (selector.options.some((option) => option.valueId === selector.currentValueId)) {
      return selector.currentValueId;
    }
    return selector.options[0]?.valueId ?? null;
  }

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
  {#if canOpen}
    <DropdownMenu.Root bind:open>
      <DropdownMenu.Trigger
        class={cn(
          'selector-btn inline-flex min-w-0 items-center gap-1 rounded px-2 py-1 text-xs text-muted-foreground transition-colors hover:bg-[var(--bg-hover)] focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40',
          triggerClass
        )}
        {disabled}
        title={`Select AI configuration (${selectedProvider?.label ?? 'Agent'})`}
      >
        <AgentIcon id={selectedProviderId ?? ''} size={12} />
        <span class="selector-label min-w-0 truncate whitespace-nowrap">{triggerLabel}</span>
        {#if configLoading}
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
        <div class="picker-column-grid">
          <div class="picker-column" data-picker-column="provider">
            <DropdownMenu.Label class="picker-section-label">Agent</DropdownMenu.Label>
            {#if agents.length > 1}
              <DropdownMenu.RadioGroup
                value={selectedProviderId ?? undefined}
                onValueChange={handleProviderChange}
              >
                {#each agents as provider (provider.id)}
                  <DropdownMenu.RadioItem value={provider.id} closeOnSelect={false}>
                    <span class="inline-flex min-w-0 items-center gap-1.5">
                      <AgentIcon id={provider.id} size={12} />
                      <span class="truncate">{provider.label}</span>
                    </span>
                  </DropdownMenu.RadioItem>
                {/each}
              </DropdownMenu.RadioGroup>
            {:else}
              <DropdownMenu.Item disabled>
                <span class="inline-flex min-w-0 items-center gap-1.5">
                  <AgentIcon id={selectedProviderId ?? ''} size={12} />
                  <span class="truncate">{selectedProvider?.label ?? 'Agent'}</span>
                </span>
              </DropdownMenu.Item>
            {/if}
          </div>

          {#if modelSelector}
            <div class="picker-column" data-picker-column="model">
              <AcpConfigPickerSection
                title={modelSelector.label || 'Model'}
                selector={modelSelector}
                value={selectedModelValue}
                onValueChange={handleModelChange}
              />
            </div>
          {/if}

          {#if effortSelector}
            <div class="picker-column" data-picker-column="effort">
              <AcpConfigPickerSection
                title={effortSelector.label || 'Effort'}
                selector={effortSelector}
                value={selectedEffortValue}
                onValueChange={handleEffortChange}
              />
            </div>
          {/if}

          {#if configLoading && !modelSelector && !effortSelector}
            <div class="picker-column" data-picker-column="status">
              <DropdownMenu.Item disabled>
                <span class="picker-status-row">
                  <Spinner size={12} />
                  Loading options…
                </span>
              </DropdownMenu.Item>
            </div>
          {/if}
        </div>

        {#if configError && !modelSelector && !effortSelector && agents.length <= 1}
          <DropdownMenu.Item disabled>
            <span class="picker-status-row">Using provider defaults</span>
          </DropdownMenu.Item>
        {/if}
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
      title={selectedProvider?.label ?? 'Agent'}
    >
      <AgentIcon id={selectedProviderId ?? ''} size={12} />
      <span class="selector-label min-w-0 truncate whitespace-nowrap"
        >{selectedProvider?.label ?? 'Agent'}</span
      >
    </button>
  {/if}
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
