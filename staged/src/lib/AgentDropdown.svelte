<!--
  AgentDropdown.svelte — TopBar dropdown for agent status and discovery.

  Shows installed agents (with a check for the selected one, clickable to
  switch), install links for missing agents, and a refresh button.
  Follows the same positioning/interaction pattern as ThemeSelectorModal.
-->
<script lang="ts">
  import { Check, ExternalLink, RefreshCw } from 'lucide-svelte';
  import { agentState, refreshProviders, KNOWN_AGENTS } from './stores/agent.svelte';
  import { preferences, setAiAgent } from './stores/preferences.svelte';
  import { openUrl } from './commands';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let dropdownRef = $state<HTMLDivElement | null>(null);
  let refreshing = $state(false);

  function isInstalled(id: string): boolean {
    return agentState.providers.some((p) => p.id === id);
  }

  function select(id: string) {
    setAiAgent(id);
  }

  async function refresh() {
    refreshing = true;
    await refreshProviders();
    refreshing = false;
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      onClose();
      event.preventDefault();
    }
  }

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as HTMLElement;
    if (dropdownRef && !dropdownRef.contains(target) && !target.closest('.agent-btn')) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleClickOutside} />

<div class="agent-dropdown" bind:this={dropdownRef}>
  <div class="dropdown-header">
    <span class="header-label">AI Agents</span>
    <button class="refresh-btn" onclick={refresh} disabled={refreshing} title="Re-scan for agents">
      <span class="spin-icon" class:spinning={refreshing}><RefreshCw size={12} /></span>
    </button>
  </div>

  <div class="agent-list">
    {#each KNOWN_AGENTS as agent (agent.id)}
      {@const installed = isInstalled(agent.id)}
      {#if installed}
        <button
          class="agent-item"
          class:active={preferences.aiAgent === agent.id}
          onclick={() => select(agent.id)}
        >
          <div class="agent-info">
            <span class="agent-name">{agent.label}</span>
            <span class="agent-status installed">Installed</span>
          </div>
          {#if preferences.aiAgent === agent.id}
            <Check size={14} />
          {/if}
        </button>
      {:else}
        <div class="agent-item unavailable">
          <div class="agent-info">
            <span class="agent-name">{agent.label}</span>
            <span class="agent-status">Not installed</span>
          </div>
          {#if agent.installUrl}
            <button
              class="install-link"
              onclick={() => openUrl(agent.installUrl!)}
              title="Install {agent.label}"
            >
              <ExternalLink size={12} />
            </button>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
</div>

<style>
  .agent-dropdown {
    position: fixed;
    top: 40px;
    right: 8px;
    z-index: 1000;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    width: 220px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .dropdown-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .header-label {
    font-size: var(--size-xs);
    font-weight: 500;
    color: var(--text-muted);
  }

  .refresh-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 3px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .refresh-btn:not(:disabled):hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .refresh-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .agent-list {
    display: flex;
    flex-direction: column;
    padding: 4px 0;
  }

  .agent-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    width: 100%;
    padding: 6px 12px;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-xs);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .agent-item:hover {
    background-color: var(--bg-hover);
  }

  .agent-item.active {
    background-color: var(--bg-primary);
  }

  .agent-item.active :global(svg) {
    color: var(--ui-accent);
  }

  .agent-item.unavailable {
    cursor: default;
    opacity: 0.6;
  }

  .agent-item.unavailable:hover {
    background: none;
  }

  .agent-info {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 0;
  }

  .agent-name {
    white-space: nowrap;
  }

  .agent-status {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
  }

  .agent-status.installed {
    color: var(--diff-added-text);
  }

  .install-link {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2px;
    background: none;
    border: none;
    color: var(--text-faint);
    border-radius: 3px;
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .install-link:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .spin-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .spin-icon.spinning {
    animation: spin 1s linear infinite;
  }
</style>
