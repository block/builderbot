<!--
  AgentSetupModal.svelte - First-run modal for selecting AI agent

  Shown on first launch when no AI agent preference has been set.
  Displays available agents and installation links for unavailable ones.
-->
<script lang="ts">
  import { RefreshCw, ExternalLink, Bot, Check } from 'lucide-svelte';
  import { discoverAcpProviders, type AcpProviderInfo } from './services/ai';
  import { setAiAgent } from './stores/preferences.svelte';
  import { openUrl } from './services/window';

  interface Props {
    onComplete: () => void;
  }

  let { onComplete }: Props = $props();

  // Known agents with metadata
  const KNOWN_AGENTS = [
    {
      id: 'goose',
      label: 'Goose',
      description: 'Open-source AI agent by Block',
      installUrl: 'https://github.com/block/goose',
    },
    {
      id: 'claude',
      label: 'Claude Code',
      description: 'AI coding assistant by Anthropic',
      installUrl: 'https://github.com/zed-industries/claude-code-acp#installation',
    },
    {
      id: 'codex',
      label: 'Codex',
      description: 'AI coding agent by OpenAI',
      installUrl: 'https://github.com/zed-industries/codex-acp#installation',
    },
  ];

  let availableProviders = $state<AcpProviderInfo[]>([]);
  let initialLoading = $state(true);
  let refreshing = $state(false);
  let selectedAgent = $state<string | null>(null);

  // Discover providers on mount
  $effect(() => {
    refreshProviders(true);
  });

  async function refreshProviders(isInitial = false) {
    if (isInitial) {
      initialLoading = true;
    } else {
      refreshing = true;
    }

    // Force UI update and paint before async work
    await new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve, 0)));

    // Ensure spinner shows for at least 400ms for better UX
    const minDisplayTime = isInitial ? 0 : 400;
    const startTime = Date.now();

    try {
      const newProviders = await discoverAcpProviders();
      // Only update if providers actually changed (avoid UI flicker)
      const currentIds = availableProviders
        .map((p) => p.id)
        .sort()
        .join(',');
      const newIds = newProviders
        .map((p) => p.id)
        .sort()
        .join(',');
      if (currentIds !== newIds) {
        availableProviders = newProviders;
      }
      // Auto-select if only one available
      if (availableProviders.length === 1 && !selectedAgent) {
        selectedAgent = availableProviders[0].id;
      }
    } catch (e) {
      console.error('Failed to discover providers:', e);
      availableProviders = [];
    } finally {
      // Wait for minimum display time before hiding spinner
      const elapsed = Date.now() - startTime;
      if (elapsed < minDisplayTime) {
        await new Promise((resolve) => setTimeout(resolve, minDisplayTime - elapsed));
      }
      initialLoading = false;
      refreshing = false;
    }
  }

  function isAvailable(agentId: string): boolean {
    return availableProviders.some((p) => p.id === agentId);
  }

  function handleContinue() {
    if (selectedAgent) {
      setAiAgent(selectedAgent);
      onComplete();
    }
  }

  function openInstallUrl(url: string) {
    openUrl(url);
  }
</script>

<div class="modal-backdrop" role="dialog" aria-modal="true" tabindex="-1">
  <div class="modal">
    <header class="modal-header">
      <div class="header-icon">
        <Bot size={24} />
      </div>
      <h2>Choose Your AI Agent</h2>
      <p class="subtitle">This will power chat, code generation and analysis</p>
    </header>

    <div class="modal-body">
      {#if initialLoading}
        <div class="loading">
          <span class="spin-container"><RefreshCw size={20} /></span>
          <span>Detecting installed agents...</span>
        </div>
      {:else}
        <div class="agents-list">
          {#each KNOWN_AGENTS as agent}
            {@const available = isAvailable(agent.id)}
            <div class="agent-row">
              <button
                class="agent-card"
                class:available
                class:selected={selectedAgent === agent.id}
                disabled={!available}
                onclick={() => available && (selectedAgent = agent.id)}
              >
                <div class="agent-info">
                  <div class="agent-header">
                    <span class="agent-name">{agent.label}</span>
                    {#if !available}
                      <span class="status-badge unavailable">Not installed</span>
                    {/if}
                  </div>
                  <p class="agent-description">{agent.description}</p>
                </div>
                {#if available}
                  <div class="check-indicator">
                    {#if selectedAgent === agent.id}
                      <Check size={16} />
                    {/if}
                  </div>
                {/if}
              </button>
              {#if !available}
                <button class="install-btn" onclick={() => openInstallUrl(agent.installUrl)}>
                  <ExternalLink size={12} />
                  Install
                </button>
              {/if}
            </div>
          {/each}
        </div>

        {#if availableProviders.length === 0}
          <div class="no-agents-hint">Install an AI agent and click Refresh to continue</div>
        {/if}
      {/if}
    </div>

    <footer class="modal-footer">
      <button class="refresh-btn" onclick={() => refreshProviders()} disabled={refreshing}>
        <span class="spin-container" class:spinning={refreshing}><RefreshCw size={14} /></span>
        {refreshing ? 'Refreshing...' : 'Refresh'}
      </button>
      <button class="continue-btn" onclick={handleContinue} disabled={!selectedAgent || refreshing}>
        Continue
      </button>
    </footer>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 440px;
    max-width: 90vw;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    padding: 24px 24px 16px;
    text-align: center;
  }

  .header-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    margin: 0 auto 12px;
    background: var(--bg-primary);
    border-radius: 12px;
    color: var(--ui-accent);
  }

  .modal-header h2 {
    margin: 0 0 4px 0;
    font-size: calc(var(--size-base) + 2px);
    font-weight: 600;
    color: var(--text-primary);
  }

  .subtitle {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .modal-body {
    padding: 0 24px;
  }

  .loading {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 32px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .agents-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .agent-row {
    display: flex;
    align-items: stretch;
    gap: 8px;
  }

  .agent-card {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: var(--bg-primary);
    border: 2px solid transparent;
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition:
      border-color 0.15s,
      background-color 0.15s;
  }

  .agent-card:not(:disabled):hover {
    background: var(--bg-hover);
  }

  .agent-card.available {
    cursor: pointer;
  }

  .agent-card.selected {
    border-color: var(--ui-accent);
    background: var(--bg-hover);
  }

  .agent-card:disabled {
    cursor: default;
    opacity: 0.7;
  }

  .agent-info {
    flex: 1;
  }

  .agent-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 2px;
  }

  .agent-name {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .status-badge {
    font-size: calc(var(--size-xs) - 1px);
    padding: 2px 6px;
    border-radius: 4px;
    font-weight: 500;
  }

  .status-badge.unavailable {
    background: var(--bg-chrome);
    color: var(--text-faint);
  }

  .agent-description {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .check-indicator {
    width: 24px;
    height: 24px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--ui-accent);
  }

  .install-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 10px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .install-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .no-agents-hint {
    margin-top: 12px;
    padding: 12px;
    background: var(--bg-primary);
    border-radius: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
    text-align: center;
  }

  .modal-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 24px;
    margin-top: 8px;
  }

  .refresh-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .refresh-btn:not(:disabled):hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
  }

  .refresh-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .continue-btn {
    padding: 8px 20px;
    background: var(--ui-accent);
    border: none;
    border-radius: 6px;
    color: var(--bg-primary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .continue-btn:not(:disabled):hover {
    background: var(--ui-accent-hover);
  }

  .continue-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  .spin-container {
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .spin-container.spinning {
    animation: spin 1s linear infinite;
  }

  .loading .spin-container {
    animation: spin 1s linear infinite;
  }
</style>
