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
  import { listenToEvent, type UnlistenFn } from '../../transport';
  import Plus from '@lucide/svelte/icons/plus';
  import X from '@lucide/svelte/icons/x';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import Ban from '@lucide/svelte/icons/ban';
  import Eye from '@lucide/svelte/icons/eye';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import type { Session, SessionStatus, SessionStatusPayload } from '../../types';
  import { startSession, deleteSession } from '../../api/commands';
  import SessionModal from './SessionModal.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { toast } from 'svelte-sonner';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';
  import { Input } from '$lib/components/ui/input';
  import { Button } from '$lib/components/ui/button';

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

  let unlistenStatus: UnlistenFn | null = null;

  // =========================================================================
  // Lifecycle — listen for status events
  // =========================================================================

  onMount(() => {
    unlistenStatus = listenToEvent<SessionStatusPayload>('session-status-changed', (payload) => {
      const { sessionId, status, errorMessage, completionReason } = payload;
      sessions = sessions.map((s) => {
        if (s.id === sessionId) {
          return {
            ...s,
            status: status as SessionStatus,
            errorMessage: errorMessage ?? s.errorMessage,
            completionReason: completionReason ?? s.completionReason,
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
      // Register the session in the unified registry
      // Note: SessionLauncher creates standalone sessions not tied to any project/branch
      // We use a placeholder projectId since the registry requires it
      sessionRegistry.register(s.id, 'standalone', 'other');
      sessions = [...sessions, s];
      prompt = '';
    } catch (e) {
      toast.error('Unable to start session', {
        description: e instanceof Error ? e.message : String(e),
        duration: Infinity,
      });
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
    try {
      await deleteSession(id);
      closeModal(id);
      sessions = sessions.filter((s) => s.id !== id);
    } catch (e) {
      toast.error('Unable to delete session', {
        description: e instanceof Error ? e.message : String(e),
        duration: Infinity,
      });
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
    if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
      e.preventDefault();
      handleCreate();
    }
  }
</script>

<!-- Floating panel — not a modal, doesn't block interaction -->
<div class="launcher">
  <div class="launcher-header">
    <span class="launcher-title">Sessions</span>
    <Button
      variant="ghost"
      size="icon"
      class="size-7 rounded-md text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-3.5"
      title="Close"
      aria-label="Close"
      onclick={onClose}
    >
      <X size={14} />
    </Button>
  </div>

  <!-- Create form -->
  <div class="create-row">
    <Input
      type="text"
      placeholder="Session prompt…"
      bind:value={prompt}
      onkeydown={handleKeydown}
      disabled={creating}
      class="flex-1"
    />
    <span class="inline-flex" title="Create session">
      <Button
        variant="ghost"
        size="icon"
        class="size-7 rounded-md text-[var(--ui-accent)] hover:bg-[var(--bg-hover)] hover:text-[var(--ui-accent)] [&_svg]:!size-3.5"
        aria-label="Create session"
        onclick={handleCreate}
        disabled={creating || !prompt.trim()}
      >
        {#if creating}
          <Spinner size={14} />
        {:else}
          <Plus size={14} />
        {/if}
      </Button>
    </span>
  </div>

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
            <span class="inline-flex" title="Open session viewer">
              <Button
                variant="ghost"
                size="icon"
                class="size-[22px] rounded text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground [&_svg]:!size-3"
                aria-label="Open session viewer"
                onclick={() => openModal(s.id)}
                disabled={openModals.has(s.id)}
              >
                <Eye size={12} />
              </Button>
            </span>
            <Button
              variant="ghost"
              size="icon"
              class="size-[22px] rounded text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-destructive [&_svg]:!size-3"
              title="Delete session"
              aria-label="Delete session"
              onclick={() => handleDelete(s.id)}
            >
              <Trash2 size={12} />
            </Button>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    <div class="empty-hint">Create a session to get started</div>
  {/if}
</div>

<!-- Open SessionModals — one per open session, stacked.
     Each modal is keyed and conditionally mounted by {#each}; bits-ui exit
     animations don't play here since closeModal removes the entry outright. -->
{#each [...openModals] as modalSessionId (modalSessionId)}
  <SessionModal
    open={true}
    sessionId={modalSessionId}
    repoDir={null}
    onClose={() => closeModal(modalSessionId)}
  />
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
    color: var(--status-added);
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

  .empty-hint {
    padding: 16px 14px;
    text-align: center;
    color: var(--text-faint);
    font-size: var(--size-xs);
  }
</style>
