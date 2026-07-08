<!--
  AcpConfigPicker.svelte — Compact provider/model/effort picker.

  Provider changes are persisted to the existing recent-agent preference so
  other launch paths keep using the selected provider. New-session launch paths
  can subscribe to the provider plus selected model/effort payload.
-->
<script lang="ts">
  import AgentIcon from './AgentIcon.svelte';
  import AcpConfigPickerSection from './AcpConfigPickerSection.svelte';
  import AcpConfigPickerShell from './AcpConfigPickerShell.svelte';
  import { agentState, REMOTE_AGENTS } from './agent.svelte';
  import { setAiAgent, getPreferredAgent } from '../settings/preferences.svelte';
  import {
    discoverAcpConfig,
    type AcpConfigDiscovery,
    type AcpConfigSelector,
  } from '../../api/commands';
  import Spinner from '../../shared/Spinner.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { buildAcpConfigSelection, type AcpConfigPickerSelection } from './acpConfigSelection';

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
  let retainedEffortSelector = $state<AcpConfigSelector | null>(null);
  let retainedEffortValue = $state<string | null>(null);
  let retainedEffortTriggerLabel = $state<string | null>(null);
  let discoveryRun = 0;

  let agents = $derived<AgentOption[]>(remote ? REMOTE_AGENTS : agentState.providers);
  let selectedProviderId = $derived(getPreferredAgent(agents));
  let selectedProvider = $derived(agents.find((provider) => provider.id === selectedProviderId));
  let modelSelector = $derived(config?.model ?? null);
  let effortSelector = $derived(config?.effort ?? null);
  let loadingEffortOptions = $derived(configLoading && !!modelSelector && !effortSelector);
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
      : (selectedProvider?.label ?? 'Agent')
  );
  let renderedTriggerParts = $derived(
    triggerParts.length > 0
      ? triggerParts
      : [{ id: 'provider', label: selectedProvider?.label ?? 'Agent' }]
  );
  // Only spin the trigger before any model/effort content is available; once
  // values are showing, reloads keep the chevron instead of flickering.
  let triggerLoading = $derived(configLoading && triggerParts.length === 0);
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
    selectedModelValue = null;
    selectedEffortValue = null;
    modelSelectionExplicit = false;
    effortSelectionExplicit = false;
    modelSelectorKey = null;
    effortSelectorKey = null;
    retainedEffortSelector = null;
    retainedEffortValue = null;
    retainedEffortTriggerLabel = null;
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
      if (!(modelSelectionExplicit && selectorHasValue(modelSelector, selectedModelValue))) {
        selectedModelValue = defaultSelectorValue(modelSelector);
        modelSelectionExplicit = false;
      }
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

  // Snapshot the effort options so a model change can keep showing the old
  // column as muted text while the model-specific options load.
  $effect(() => {
    if (!configLoading && effortSelector) {
      retainedEffortSelector = effortSelector;
      retainedEffortValue = selectedEffortValue;
      retainedEffortTriggerLabel = effortTriggerLabel;
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
    selectedEffortValue = null;
    effortSelectionExplicit = false;
    config = config ? { ...config, effort: null } : config;

    const providerId = selectedProviderId;
    const discoveryWorkingDir = workingDir ?? null;
    if (remote || !providerId) return;

    const run = ++discoveryRun;
    configLoading = true;
    configError = null;

    discoverAcpConfig(providerId, discoveryWorkingDir, { selectedModelValue: value })
      .then(({ data, revalidating }) => {
        if (run !== discoveryRun) return;
        config = data;
        configLoading = false;
        revalidating
          ?.then((fresh) => {
            if (run === discoveryRun) {
              config = fresh;
            }
          })
          .catch((error) => {
            if (run === discoveryRun) {
              console.error('Failed to revalidate ACP config for selected model:', error);
            }
          });
      })
      .catch((error) => {
        if (run !== discoveryRun) return;
        console.error('Failed to discover ACP config for selected model:', error);
        configLoading = false;
        configError = error instanceof Error ? error.message : String(error);
      });
  }

  function handleEffortChange(value: string) {
    selectedEffortValue = value;
    effortSelectionExplicit = true;
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

  function selectorHasValue(selector: AcpConfigSelector | null, valueId: string | null): boolean {
    return !!selector && !!valueId && selector.options.some((option) => option.valueId === valueId);
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
  <AcpConfigPickerShell
    providerId={selectedProviderId}
    {triggerLabel}
    triggerParts={renderedTriggerParts}
    triggerTitle={canOpen
      ? `Select AI configuration (${selectedProvider?.label ?? 'Agent'})`
      : (selectedProvider?.label ?? 'Agent')}
    loading={triggerLoading}
    {disabled}
    {dropUp}
    {triggerClass}
    {canOpen}
  >
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
    {:else if loadingEffortOptions}
      <div class="picker-column" data-picker-column="effort">
        {#if retainedEffortSelector}
          <AcpConfigPickerSection
            title={retainedEffortSelector.label || 'Effort'}
            selector={retainedEffortSelector}
            value={retainedEffortValue}
            disabled
            onValueChange={() => {}}
          />
        {:else}
          <DropdownMenu.Label class="picker-section-label">Effort</DropdownMenu.Label>
          <DropdownMenu.Item disabled>
            <span class="picker-status-row">
              <Spinner size={12} />
              <span class="picker-status-text">Loading options…</span>
            </span>
          </DropdownMenu.Item>
        {/if}
      </div>
    {/if}

    {#if configLoading && !loadingEffortOptions && (!modelSelector || !effortSelector)}
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
      {#if configError && !modelSelector && !effortSelector && agents.length <= 1}
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">Using provider defaults</span>
        </DropdownMenu.Item>
      {/if}
    {/snippet}
  </AcpConfigPickerShell>
{/if}
