<!--
  AgentSetupModal.svelte — Shown when no ACP agents are detected.

  Informs the user that AI features require an installed agent and
  provides install links. Has a Refresh button to re-scan and a
  Dismiss button so the user isn't blocked from using the rest of
  the app.

  Auto-closes when an agent is detected after refresh.
-->
<script lang="ts">
  import { RefreshCw, ExternalLink, Bot, X } from 'lucide-svelte';
  import { agentState, refreshProviders, KNOWN_AGENTS } from './stores/agent.svelte';
  import { openUrl } from './commands';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  let refreshing = $state(false);

  // Auto-close when agents become available after a refresh.
  $effect(() => {
    if (agentState.providers.length > 0) {
      onClose();
    }
  });

  async function refresh() {
    refreshing = true;
    await refreshProviders();
    refreshing = false;
  }
</script>

<div class="modal-backdrop" role="dialog" aria-modal="true" tabindex="-1">
  <div class="modal">
    <button class="close-btn" onclick={onClose} title="Dismiss">
      <X size={16} />
    </button>

    <header class="modal-header">
      <div class="header-icon">
        <Bot size={24} />
      </div>
      <h2>No AI Agent Detected</h2>
      <p class="subtitle">
        Most Staged features need an AI agent to work. Install one of the following and click <strong
          >Refresh</strong
        >.
      </p>
    </header>

    <div class="modal-body">
      <div class="agents-list">
        {#each KNOWN_AGENTS as agent (agent.id)}
          <div class="agent-row">
            <div class="agent-info">
              <span class="agent-name">{agent.label}</span>
              <p class="agent-description">{agent.description}</p>
            </div>
            {#if agent.installUrl}
              <button class="install-link" onclick={() => openUrl(agent.installUrl!)}>
                <ExternalLink size={12} />
                Install
              </button>
            {/if}
          </div>
        {/each}
      </div>
    </div>

    <footer class="modal-footer">
      <button class="refresh-btn" onclick={refresh} disabled={refreshing}>
        <span class="spin-icon" class:spinning={refreshing}><RefreshCw size={14} /></span>
        {refreshing ? 'Checking…' : 'Refresh'}
      </button>
      <button class="dismiss-btn" onclick={onClose}>Dismiss</button>
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
    position: relative;
    background: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    width: 420px;
    max-width: 90vw;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .close-btn {
    position: absolute;
    top: 12px;
    right: 12px;
    background: none;
    border: none;
    color: var(--text-faint);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    transition: color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
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
    color: var(--text-muted);
  }

  .modal-header h2 {
    margin: 0 0 6px 0;
    font-size: calc(var(--size-base) + 2px);
    font-weight: 600;
    color: var(--text-primary);
  }

  .subtitle {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    line-height: 1.4;
  }

  .modal-body {
    padding: 0 24px;
  }

  .agents-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .agent-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    background: var(--bg-primary);
    border-radius: 8px;
  }

  .agent-info {
    flex: 1;
    min-width: 0;
  }

  .agent-name {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .agent-description {
    margin: 2px 0 0 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .install-link {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-family: inherit;
    white-space: nowrap;
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .install-link:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
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
    font-family: inherit;
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

  .dismiss-btn {
    padding: 8px 16px;
    background: none;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    font-family: inherit;
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .dismiss-btn:hover {
    color: var(--text-primary);
    border-color: var(--border-emphasis);
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
