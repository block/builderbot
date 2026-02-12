<!--
  SessionLauncher.svelte — Hidden dev panel for spawning standalone sessions

  Triggered via konami code. A persistent floating panel anchored to the
  bottom-right so it doesn't block the main UI. Lets you:
  - Create new unaffiliated sessions (just a prompt)
  - See a list of sessions with status (updated via events)
  - Open a SessionModal for any of them (multiple at once)
  - Delete sessions from the database
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen, type UnlistenFn } from '@tauri-apps/api/event';
  import { Plus, X, CheckCircle, AlertCircle, Ban, Eye, Trash2 } from 'lucide-svelte';
  import type { Session, SessionStatus } from '../../types';
  import { startSession, deleteSession } from '../../commands';
  import SessionModal from './SessionModal.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { getPreferredAgent } from '../settings/preferences.svelte';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  // =========================================================================
  // State
  // =========================================================================

  /** All sessions we've created in this launcher instance */
  let sessions = $state<Session[]>([]);

  /** Set of session IDs that have an open modal */
  let openModals = $state<Set<string>>(new Set());

  let prompt = $state('');
  let creating = $state(false);
  let error = $state<string | null>(null);

  let unlistenStatus: UnlistenFn | null = null;

  // =========================================================================
  // Lifecycle — listen for status events
  // =========================================================================

  onMount(async () => {
    unlistenStatus = await listen<{
      sessionId: string;
      status: string;
      errorMessage: string | null;
    }>('session-status-changed', (event) => {
      const { sessionId, status, errorMessage } = event.payload;
      sessions = sessions.map((s) => {
        if (s.id === sessionId) {
          return {
            ...s,
            status: status as SessionStatus,
            errorMessage: errorMessage ?? s.errorMessage,
            updatedAt: Date.now(),
          };
        }
        return s;
      });
    });
  });

  onDestroy(() => {
    unlistenStatus?.();
  });

  // =========================================================================
  // Actions
  // =========================================================================

  async function handleCreate() {
    const text = prompt.trim();
    if (!text || creating) return;
    creating = true;
    error = null;
    try {
      // startSession creates the session + kicks off goose in the background.
      // We need a working directory — use the user's home dir as a default
      // for these standalone debug sessions.
      const workingDir = '/tmp';
      const s = await startSession(
        text,
        workingDir,
        getPreferredAgent(agentState.providers) ?? undefined
      );
      sessions = [...sessions, s];
      prompt = '';
    } catch (e) {
      error = `Failed to start: ${e}`;
    } finally {
      creating = false;
    }
  }

  function openModal(id: string) {
    openModals = new Set([...openModals, id]);
  }

  function closeModal(id: string) {
    const next = new Set(openModals);
    next.delete(id);
    openModals = next;
  }

  async function handleDelete(id: string) {
    error = null;
    try {
      await deleteSession(id);
      closeModal(id);
      sessions = sessions.filter((s) => s.id !== id);
    } catch (e) {
      error = `Failed to delete: ${e}`;
    }
  }

  // =========================================================================
  // Helpers
  // =========================================================================

  function statusIcon(status: SessionStatus) {
    switch (status) {
      case 'completed':
        return CheckCircle;
      case 'error':
        return AlertCircle;
      case 'cancelled':
        return Ban;
      default:
        return null;
    }
  }

  function statusClass(status: SessionStatus): string {
    return `status-${status}`;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleCreate();
    }
  }
</script>

<!-- Floating panel — not a modal, doesn't block interaction -->
<div class="launcher">
  <div class="launcher-header">
    <span class="launcher-title">Sessions</span>
    <button class="icon-btn" onclick={onClose} title="Close">
      <X size={14} />
    </button>
  </div>

  <!-- Create form -->
  <div class="create-row">
    <input
      type="text"
      class="create-input"
      placeholder="Session prompt…"
      bind:value={prompt}
      onkeydown={handleKeydown}
      disabled={creating}
    />
    <button
      class="icon-btn accent"
      onclick={handleCreate}
      disabled={creating || !prompt.trim()}
      title="Create session"
    >
      {#if creating}
        <Spinner size={14} />
      {:else}
        <Plus size={14} />
      {/if}
    </button>
  </div>

  {#if error}
    <div class="error-line">{error}</div>
  {/if}

  <!-- Session list -->
  {#if sessions.length > 0}
    <div class="session-list">
      {#each sessions as s (s.id)}
        {@const Icon = statusIcon(s.status)}
        <div class="session-row">
          <div class="session-indicator {statusClass(s.status)}">
            {#if s.status === 'running'}
              <Spinner size={12} />
            {:else if Icon}
              <Icon size={12} />
            {/if}
          </div>
          <div class="session-info">
            <span class="session-prompt">{s.prompt}</span>
            <span class="session-id">{s.id.slice(0, 8)}</span>
          </div>
          <div class="session-actions">
            <button
              class="mini-btn"
              onclick={() => openModal(s.id)}
              title="Open session viewer"
              disabled={openModals.has(s.id)}
            >
              <Eye size={12} />
            </button>
            <button
              class="mini-btn danger"
              onclick={() => handleDelete(s.id)}
              title="Delete session"
            >
              <Trash2 size={12} />
            </button>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    <div class="empty-hint">Create a session to get started</div>
  {/if}
</div>

<!-- Open SessionModals — one per open session, stacked -->
{#each [...openModals] as modalSessionId (modalSessionId)}
  <SessionModal sessionId={modalSessionId} onClose={() => closeModal(modalSessionId)} />
{/each}

<style>
  .launcher {
    position: fixed;
    bottom: 16px;
    right: 16px;
    width: 340px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-subtle);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    z-index: 900;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .launcher-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .launcher-title {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
  }

  /* Create row */
  .create-row {
    display: flex;
    gap: 6px;
    padding: 10px 14px;
  }

  .create-input {
    flex: 1;
    padding: 6px 10px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-xs);
  }

  .create-input::placeholder {
    color: var(--text-faint);
  }

  .create-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  /* Session list */
  .session-list {
    max-height: 240px;
    overflow-y: auto;
    padding: 0 6px 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .session-row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
    transition: background-color 0.1s;
  }

  .session-row:hover {
    background: var(--bg-hover);
  }

  .session-indicator {
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }

  .session-indicator.status-running {
    color: var(--ui-accent);
  }
  .session-indicator.status-completed {
    color: var(--ui-success, #22c55e);
  }
  .session-indicator.status-error {
    color: var(--ui-danger);
  }
  .session-indicator.status-cancelled {
    color: var(--text-muted);
  }

  .session-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .session-prompt {
    font-size: var(--size-xs);
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .session-id {
    font-size: 10px;
    color: var(--text-faint);
    font-family: 'SF Mono', 'Menlo', monospace;
  }

  .session-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }

  /* Buttons */
  .icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .icon-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .icon-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .icon-btn.accent {
    color: var(--ui-accent);
  }

  .icon-btn.accent:hover {
    background: var(--bg-hover);
  }

  .mini-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .mini-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .mini-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .mini-btn.danger:hover {
    color: var(--ui-danger);
  }

  .error-line {
    padding: 4px 14px 8px;
    font-size: var(--size-2xs, 10px);
    color: var(--ui-danger);
  }

  .empty-hint {
    padding: 16px 14px;
    text-align: center;
    color: var(--text-faint);
    font-size: var(--size-xs);
  }
</style>
