<!--
  DiagramConfigSetting.svelte — settings control for the diagram-generation
  sub-session's agent, model, and effort.

  The `generate_pikchr` tool spins up its own sub-session; by default it inherits
  the invoking session's agent at that agent's default model/effort. This control
  lets the user pin the diagram sub-session to a specific agent — and that agent's
  model and effort — instead. The selection is persisted under the
  `diagram-subsession-config` preference and read by the backend when the tool runs.

  Renders through the same AcpConfigPickerShell/Section chrome as the chat pickers
  (one trigger button, up to three columns), but keeps its own discovery and
  persistence so it never disturbs the shared session provider/model/effort prefs.
-->
<script lang="ts">
  import Info from '@lucide/svelte/icons/info';
  import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
  import AgentIcon from '../agents/AgentIcon.svelte';
  import AcpConfigPickerSection from '../agents/AcpConfigPickerSection.svelte';
  import AcpConfigPickerShell from '../agents/AcpConfigPickerShell.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { preferences, setDiagramSubsessionConfig } from './preferences.svelte';
  import {
    discoverAcpConfig,
    type AcpConfigDiscovery,
    type AcpConfigSelector,
  } from '../../api/commands';
  import { defaultSelectorValue, selectorHasValue } from '../agents/acpConfigSelection';
  import Spinner from '../../shared/Spinner.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';

  /** Sentinel value for the "inherit the invoking session's agent" choice. */
  const SAME_AS_SESSION = '__same_as_session__';

  let config = $state<AcpConfigDiscovery | null>(null);
  let configLoading = $state(false);
  let configError = $state<string | null>(null);
  let discoveryRun = 0;

  const providers = $derived(agentState.providers);
  const selectedProvider = $derived(preferences.diagramSubsessionConfig?.provider ?? null);
  const selectedModel = $derived(preferences.diagramSubsessionConfig?.model ?? null);
  const selectedEffort = $derived(preferences.diagramSubsessionConfig?.effort ?? null);

  const modelSelector = $derived(config?.model ?? null);
  const effortSelector = $derived(config?.effort ?? null);

  const selectedProviderInfo = $derived(
    providers.find((provider) => provider.id === selectedProvider)
  );
  const providerTriggerLabel = $derived(
    selectedProvider ? (selectedProviderInfo?.label ?? selectedProvider) : 'Same as session'
  );

  // Discover the chosen agent's model/effort options (and, once a model is
  // stored, that model's effort options). Re-runs whenever the provider or the
  // stored model changes; ignored while no specific agent is selected.
  $effect(() => {
    const providerId = selectedProvider;
    const modelValue = selectedModel?.valueId ?? null;
    if (!providerId) {
      config = null;
      configLoading = false;
      configError = null;
      return;
    }

    const run = ++discoveryRun;
    let cancelled = false;
    configLoading = true;
    configError = null;

    discoverAcpConfig(providerId, null, modelValue ? { selectedModelValue: modelValue } : {})
      .then(({ data }) => {
        if (cancelled || run !== discoveryRun) return;
        config = data;
        configLoading = false;
      })
      .catch((error) => {
        if (cancelled || run !== discoveryRun) return;
        console.error('Failed to discover diagram ACP config:', error);
        config = null;
        configLoading = false;
        configError = error instanceof Error ? error.message : String(error);
      });

    return () => {
      cancelled = true;
    };
  });

  // The stored choice can go stale out from under the preference: the agent
  // can be uninstalled, or an agent update can drop the pinned model/effort
  // value id. The backend then falls back to the invoking session's agent per
  // call (the preference itself is kept, so a later agent update can revive
  // it) — surface that as an error here instead of silently rendering the
  // agent default as if it were the active choice.
  const providerMissing = $derived(
    !!selectedProvider && agentState.loaded && !selectedProviderInfo
  );
  const modelStale = $derived(
    !configLoading &&
      !!modelSelector &&
      !!selectedModel &&
      !selectorHasValue(modelSelector, selectedModel.valueId)
  );
  const effortStale = $derived(
    !configLoading &&
      !!effortSelector &&
      !!selectedEffort &&
      !selectorHasValue(effortSelector, selectedEffort.valueId)
  );
  const configUnavailable = $derived(providerMissing || modelStale || effortStale);
  const staleSelectionNoun = $derived(
    modelStale && effortStale ? 'model and effort' : modelStale ? 'model' : 'effort'
  );

  // Reconcile the stored value id against the live options for the column
  // highlight (options vary by agent version); a stale id highlights the
  // agent's default as the value a re-pick would start from.
  const modelDisplayValue = $derived(
    selectorHasValue(modelSelector, selectedModel?.valueId ?? null)
      ? (selectedModel?.valueId ?? null)
      : defaultSelectorValue(modelSelector)
  );
  const effortDisplayValue = $derived(
    selectorHasValue(effortSelector, selectedEffort?.valueId ?? null)
      ? (selectedEffort?.valueId ?? null)
      : defaultSelectorValue(effortSelector)
  );

  // The trigger keeps showing the stale stored label (not the reconciled
  // default) so the error text below reads against what was actually chosen.
  const modelTriggerLabel = $derived(
    modelStale
      ? (selectedModel?.label ?? selectedModel?.valueId ?? null)
      : selectorValueLabel(modelSelector, modelDisplayValue)
  );
  const effortTriggerLabel = $derived(
    effortStale
      ? (selectedEffort?.label ?? selectedEffort?.valueId ?? null)
      : selectorValueLabel(effortSelector, effortDisplayValue)
  );
  const loadingEffortOptions = $derived(configLoading && !!modelSelector && !effortSelector);

  // The provider is carried by the trigger icon; the two text rows show the
  // model and effort (matching the in-session picker), falling back to the
  // agent name before discovery lands.
  const triggerParts = $derived(
    [
      modelTriggerLabel ? { id: 'model', label: modelTriggerLabel } : null,
      effortTriggerLabel ? { id: 'effort', label: effortTriggerLabel } : null,
    ].filter((part): part is { id: string; label: string } => !!part)
  );
  const triggerLabel = $derived(
    triggerParts.length > 0
      ? triggerParts.map((part) => part.label).join(' · ')
      : providerTriggerLabel
  );
  const triggerLoading = $derived(configLoading && triggerParts.length === 0);
  const canOpen = $derived(
    providers.length > 0 || !!selectedProvider || configLoading || !!configError
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

  function handleProviderChange(value: string) {
    if (value === SAME_AS_SESSION) {
      setDiagramSubsessionConfig(null);
      return;
    }
    // Switching agents drops the previous agent's model/effort — their value ids
    // aren't portable — so they re-derive from the new agent's defaults.
    setDiagramSubsessionConfig({ provider: value });
  }

  function handleModelChange(value: string) {
    const selector = modelSelector;
    if (!selector || !selectedProvider) return;
    const option = selector.options.find((candidate) => candidate.valueId === value);
    setDiagramSubsessionConfig({
      provider: selectedProvider,
      model: { configId: selector.configId, valueId: value, label: option?.label ?? null },
      // Effort options depend on the model; clear so effort re-derives from the
      // newly selected model's defaults.
      effort: null,
    });
  }

  function handleEffortChange(value: string) {
    const selector = effortSelector;
    if (!selector || !selectedProvider) return;
    const option = selector.options.find((candidate) => candidate.valueId === value);
    setDiagramSubsessionConfig({
      provider: selectedProvider,
      model: selectedModel,
      effort: { configId: selector.configId, valueId: value, label: option?.label ?? null },
    });
  }
</script>

<div class="diagram-config-field">
  <span class="field-label">Diagram generation</span>

  <AcpConfigPickerShell
    providerId={selectedProvider}
    {triggerLabel}
    {triggerParts}
    triggerTitle={canOpen ? 'Select the agent for diagram generation' : providerTriggerLabel}
    loading={triggerLoading}
    layout="vertical"
    {canOpen}
    triggerClass={`max-w-[260px] border bg-[var(--bg-elevated)] ${
      configUnavailable ? 'border-[var(--ui-danger)]' : 'border-[var(--border-muted)]'
    }`}
  >
    <div class="picker-column" data-picker-column="provider">
      <DropdownMenu.Label class="picker-section-label">Agent</DropdownMenu.Label>
      <DropdownMenu.RadioGroup
        value={selectedProvider ?? SAME_AS_SESSION}
        onValueChange={handleProviderChange}
      >
        <DropdownMenu.RadioItem value={SAME_AS_SESSION} closeOnSelect={false}>
          <span class="truncate">Same as session</span>
        </DropdownMenu.RadioItem>
        {#each providers as provider (provider.id)}
          <DropdownMenu.RadioItem value={provider.id} closeOnSelect={false}>
            <span class="inline-flex min-w-0 items-center gap-1.5">
              <AgentIcon id={provider.id} size={12} />
              <span class="truncate">{provider.label}</span>
            </span>
          </DropdownMenu.RadioItem>
        {/each}
      </DropdownMenu.RadioGroup>
    </div>

    {#if selectedProvider}
      {#if modelSelector}
        <div class="picker-column" data-picker-column="model">
          <AcpConfigPickerSection
            title={modelSelector.label || 'Model'}
            selector={modelSelector}
            value={modelDisplayValue}
            onValueChange={handleModelChange}
          />
        </div>
      {/if}

      {#if effortSelector}
        <div class="picker-column" data-picker-column="effort">
          <AcpConfigPickerSection
            title={effortSelector.label || 'Effort'}
            selector={effortSelector}
            value={effortDisplayValue}
            onValueChange={handleEffortChange}
          />
        </div>
      {/if}

      {#if configLoading && (!modelSelector || loadingEffortOptions)}
        <div class="picker-column" data-picker-column="status">
          <DropdownMenu.Item disabled>
            <span class="picker-status-row">
              <Spinner size={12} />
              <span class="picker-status-text">Loading options…</span>
            </span>
          </DropdownMenu.Item>
        </div>
      {/if}
    {/if}

    {#snippet footer()}
      {#if providerMissing}
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">Agent no longer available</span>
        </DropdownMenu.Item>
      {:else if selectedProvider && configError && !modelSelector && !effortSelector}
        <DropdownMenu.Item disabled>
          <span class="picker-status-row">Using agent defaults</span>
        </DropdownMenu.Item>
      {/if}
    {/snippet}
  </AcpConfigPickerShell>

  <p class="field-description" class:field-description-error={configUnavailable}>
    {#if configUnavailable}
      <TriangleAlert size={12} />
    {:else}
      <Info size={12} />
    {/if}
    {#if !selectedProvider}
      Diagrams are drawn by a sub-agent using the same agent as the session that requested them.
      Pick an agent to always draw diagrams with a specific agent, model, and effort instead.
    {:else if providerMissing}
      {providerTriggerLabel} is no longer available. Diagrams will use the requesting session's own agent
      until you choose a different agent.
    {:else if modelStale || effortStale}
      {providerTriggerLabel} no longer offers the chosen {staleSelectionNoun}. Diagrams will use the
      requesting session's own agent until you update the selection.
    {:else if configError && !modelSelector && !effortSelector}
      Diagrams will use {providerTriggerLabel} at its default model and effort.
    {:else}
      Diagrams are drawn by {providerTriggerLabel}{modelTriggerLabel
        ? `, model ${modelTriggerLabel}`
        : ''}{effortTriggerLabel ? `, ${effortTriggerLabel} effort` : ''}.
    {/if}
  </p>
</div>

<style>
  .diagram-config-field {
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }

  .field-label {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  .field-description {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.4;
    display: flex;
    align-items: baseline;
    gap: 4px;
  }

  .field-description-error {
    color: var(--ui-danger);
  }
</style>
