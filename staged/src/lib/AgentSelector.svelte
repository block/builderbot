<!--
  AgentSelector.svelte - Reusable AI agent provider selector
  
  A dropdown selector for choosing the AI agent provider.
  Remembers the last selection in preferences and syncs with global provider state.
  
  Usage:
    <AgentSelector bind:provider />
-->
<script lang="ts">
  import { ChevronDown, Check } from 'lucide-svelte';
  import { onMount } from 'svelte';
  import { discoverAcpProviders, type AcpProviderInfo } from './services/ai';
  import { agentGlobalState, type AcpProvider } from './stores/agent.svelte';
  import { preferences, setAiAgent } from './stores/preferences.svelte';

  interface Props {
    /** The currently selected provider (two-way binding) */
    provider: AcpProvider;
    /** Whether the selector is disabled */
    disabled?: boolean;
  }

  let { provider = $bindable(), disabled = false }: Props = $props();

  let showDropdown = $state(false);

  /** Type guard to validate provider ID */
  function isValidProvider(id: string): id is AcpProvider {
    return id === 'goose' || id === 'claude';
  }

  // Initialize on mount: discover providers if not already loaded
  onMount(() => {
    // Discover available providers (only once globally)
    if (!agentGlobalState.providersLoaded) {
      discoverAcpProviders()
        .then((providers) => {
          agentGlobalState.availableProviders = providers;
          agentGlobalState.providersLoaded = true;

          // Use saved preference if available
          const savedAgent = preferences.aiAgent;
          if (
            savedAgent &&
            providers.some((p) => p.id === savedAgent) &&
            isValidProvider(savedAgent)
          ) {
            provider = savedAgent as AcpProvider;
          } else if (providers.length > 0 && !providers.some((p) => p.id === provider)) {
            // If current provider is not available, switch to first available valid one
            const firstValidId = providers.map((p) => p.id).find(isValidProvider);
            if (firstValidId) {
              provider = firstValidId;
            }
          }
        })
        .catch((e) => {
          console.error('Failed to discover ACP providers:', e);
        });
    } else {
      // Providers already loaded - check if we should use the saved preference
      const savedAgent = preferences.aiAgent;
      if (
        savedAgent &&
        agentGlobalState.availableProviders.some((p) => p.id === savedAgent) &&
        isValidProvider(savedAgent)
      ) {
        provider = savedAgent as AcpProvider;
      }
    }

    // Close dropdown when clicking outside
    function handleClickOutside(event: MouseEvent) {
      const target = event.target as HTMLElement;
      if (showDropdown && !target.closest('.agent-selector')) {
        showDropdown = false;
      }
    }
    document.addEventListener('click', handleClickOutside);
    return () => document.removeEventListener('click', handleClickOutside);
  });

  // Sync with preferences when they change externally
  $effect(() => {
    const savedAgent = preferences.aiAgent;
    if (
      savedAgent &&
      agentGlobalState.availableProviders.some((p) => p.id === savedAgent) &&
      isValidProvider(savedAgent)
    ) {
      provider = savedAgent as AcpProvider;
    }
  });

  function selectProvider(selectedProvider: AcpProvider) {
    provider = selectedProvider;
    showDropdown = false;
    // Save to preferences
    setAiAgent(selectedProvider);
  }

  function toggleDropdown() {
    if (!disabled) {
      showDropdown = !showDropdown;
    }
  }

  // Get the label for the current provider
  let currentLabel = $derived(
    agentGlobalState.availableProviders.find((p) => p.id === provider)?.label ?? provider
  );
</script>

{#if agentGlobalState.availableProviders.length > 0}
  <div class="agent-selector">
    <button
      type="button"
      class="selector-btn"
      onclick={toggleDropdown}
      {disabled}
      title="Select AI provider"
    >
      <span class="selector-label">{currentLabel}</span>
      <ChevronDown size={12} />
    </button>
    {#if showDropdown}
      <div class="selector-dropdown">
        {#each agentGlobalState.availableProviders as providerInfo (providerInfo.id)}
          <button
            type="button"
            class="selector-option"
            class:selected={provider === providerInfo.id}
            onclick={() => isValidProvider(providerInfo.id) && selectProvider(providerInfo.id)}
          >
            <span class="option-label">{providerInfo.label}</span>
            {#if provider === providerInfo.id}
              <Check size={14} />
            {/if}
          </button>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .agent-selector {
    position: relative;
  }

  .selector-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 6px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .selector-btn:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-muted);
  }

  .selector-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .selector-label {
    white-space: nowrap;
  }

  .selector-dropdown {
    position: absolute;
    bottom: 100%;
    left: 0;
    margin-bottom: 4px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: hidden;
    z-index: 1001;
    min-width: 120px;
  }

  .selector-option {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    padding: 8px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .selector-option:hover {
    background-color: var(--bg-hover);
  }

  .selector-option.selected {
    color: var(--text-accent);
  }

  .selector-option.selected :global(svg) {
    color: var(--text-accent);
  }

  .option-label {
    flex: 1;
  }
</style>
