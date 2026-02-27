<!--
  AgentSelector.svelte — Compact dropdown for switching the AI agent.

  Shows the currently selected agent with a chevron. Clicking opens a
  dropdown with all available providers. Selection is saved to preferences.

  Reads from the shared agentState cache (populated once at app startup)
  so there's no discovery delay when opening modals.

  Usage:
    <AgentSelector />
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { ChevronDown, Check, Bot } from 'lucide-svelte';
  import { agentState, REMOTE_AGENTS } from './agent.svelte';
  import {
    setAiAgent,
    getPreferredAgent,
    setProjectAiAgent,
    getPreferredAgentForProject,
  } from '../settings/preferences.svelte';

  interface Props {
    disabled?: boolean;
    remote?: boolean;
    projectId?: string | null;
  }

  let { disabled = false, remote = false, projectId = null }: Props = $props();

  let showDropdown = $state(false);

  let agents = $derived(remote ? REMOTE_AGENTS : agentState.providers);

  let preferredId = $derived(
    projectId ? getPreferredAgentForProject(projectId, agents) : getPreferredAgent(agents)
  );

  let currentLabel = $derived(agents.find((p) => p.id === preferredId)?.label ?? 'Agent');

  onMount(() => {
    document.addEventListener('click', handleClickOutside);
  });

  onDestroy(() => {
    document.removeEventListener('click', handleClickOutside);
  });

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (showDropdown && !target.closest('.agent-selector')) {
      showDropdown = false;
    }
  }

  function select(id: string) {
    if (projectId) {
      setProjectAiAgent(projectId, id);
    } else {
      setAiAgent(id);
    }
    showDropdown = false;
  }

  function toggle() {
    if (!disabled && agents.length > 1) {
      showDropdown = !showDropdown;
    }
  }
</script>

{#if (remote || agentState.loaded) && agents.length > 0}
  <div class="agent-selector">
    <button
      type="button"
      class="selector-btn"
      onclick={toggle}
      {disabled}
      title={agents.length > 1 ? 'Select AI agent' : currentLabel}
    >
      <Bot size={12} />
      <span class="selector-label">{currentLabel}</span>
      {#if agents.length > 1}
        <ChevronDown size={12} />
      {/if}
    </button>
    {#if showDropdown}
      <div class="selector-dropdown">
        {#each agents as provider (provider.id)}
          <button
            type="button"
            class="selector-option"
            class:selected={preferredId === provider.id}
            onclick={() => select(provider.id)}
          >
            <span class="option-label">{provider.label}</span>
            {#if preferredId === provider.id}
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
    padding: 4px 8px;
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
    min-width: 140px;
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
    color: var(--ui-accent);
  }

  .selector-option.selected :global(svg) {
    color: var(--ui-accent);
  }

  .option-label {
    flex: 1;
  }
</style>
