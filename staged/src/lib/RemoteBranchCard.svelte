<!--
  RemoteBranchCard.svelte - Card display for a remote Blox workspace branch

  Shows branch name, workspace status badge, agent type, and a prompt
  interface for interacting with the remote agent.

  Lifecycle:
  - Starting: shows spinner, polls every 3s until Running
  - Running: shows prompt input + conversation history
  - Stopped: shows restart hint
  - Error: shows error state
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import {
    GitBranch,
    Cloud,
    Loader2,
    Trash2,
    Send,
    AlertCircle,
    CircleCheck,
    CirclePause,
    Bot,
    Copy,
  } from 'lucide-svelte';
  import type { Branch, WorkspaceStatus } from './types';
  import * as commands from './commands';
  import DropdownMenu, { type MenuItem } from './DropdownMenu.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';

  interface Props {
    branch: Branch;
    deleting?: boolean;
    onDelete?: () => void;
  }

  let { branch, deleting = false, onDelete }: Props = $props();

  // Reactive workspace status (updated by polling)
  // Initialise from the prop; overwritten by poll results.
  let polledStatus = $state<WorkspaceStatus | null>(null);
  let status = $derived<WorkspaceStatus | null>(polledStatus ?? branch.workspaceStatus);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  // Prompt UI state
  let promptText = $state('');
  let sending = $state(false);
  let messages = $state<Array<{ role: 'user' | 'assistant'; content: string }>>([]);
  let messagesEl: HTMLDivElement | null = $state(null);

  // Error state
  let error = $state<string | null>(null);

  // Confirm delete dialog
  let confirmDelete = $state<{
    title: string;
    message: string;
    onConfirm: () => void;
  } | null>(null);

  const menuItems: MenuItem[] = $derived([
    ...(branch.workspaceName
      ? [
          {
            label: 'Copy Workspace Name',
            icon: Copy,
            action: () => copyText(branch.workspaceName!),
          },
        ]
      : []),
    {
      label: 'Delete Branch',
      icon: Trash2,
      danger: true,
      action: () => {
        confirmDelete = {
          title: 'Delete Remote Branch',
          message:
            'This will delete the Blox workspace and remove the branch. This action cannot be undone.',
          onConfirm: async () => {
            confirmDelete = null;
            onDelete?.();
          },
        };
      },
    },
  ]);

  async function copyText(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // clipboard API may fail
    }
  }

  // =========================================================================
  // Status polling
  // =========================================================================

  onMount(() => {
    if (status === 'starting') {
      startPolling();
    }
  });

  onDestroy(() => {
    stopPolling();
  });

  function startPolling() {
    stopPolling();
    pollTimer = setInterval(async () => {
      try {
        const newStatus = (await commands.pollWorkspaceStatus(branch.id)) as WorkspaceStatus;
        polledStatus = newStatus;
        if (newStatus !== 'starting') {
          stopPolling();
        }
      } catch (e) {
        console.error('Failed to poll workspace status:', e);
        polledStatus = 'error';
        error = e instanceof Error ? e.message : String(e);
        stopPolling();
      }
    }, 3000);
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  // =========================================================================
  // Prompt handling
  // =========================================================================

  async function handleSendPrompt() {
    const text = promptText.trim();
    if (!text || sending || status !== 'running') return;

    sending = true;
    error = null;
    messages = [...messages, { role: 'user', content: text }];
    promptText = '';
    scrollToBottom();

    try {
      const response = await commands.sendWorkspacePrompt(branch.id, text);
      messages = [...messages, { role: 'assistant', content: response }];
      scrollToBottom();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      // Remove the user message on error so they can retry
      messages = messages.slice(0, -1);
      promptText = text;
    } finally {
      sending = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSendPrompt();
    }
  }

  function scrollToBottom() {
    requestAnimationFrame(() => {
      if (messagesEl) {
        messagesEl.scrollTop = messagesEl.scrollHeight;
      }
    });
  }

  // =========================================================================
  // Display helpers
  // =========================================================================

  function statusLabel(s: WorkspaceStatus | null): string {
    switch (s) {
      case 'starting':
        return 'Starting';
      case 'running':
        return 'Running';
      case 'stopped':
        return 'Stopped';
      case 'error':
        return 'Error';
      default:
        return 'Unknown';
    }
  }

  function agentLabel(agent: string | null): string {
    if (!agent) return 'Agent';
    return agent.charAt(0).toUpperCase() + agent.slice(1);
  }
</script>

<div class="branch-card remote" class:deleting>
  {#if deleting}
    <div class="deleting-overlay">
      <Loader2 size={16} class="spinner" />
      <span>Deleting…</span>
    </div>
  {:else}
    <!-- Header -->
    <div class="card-header">
      <div class="branch-info">
        <Cloud size={16} class="cloud-icon" />
        <span class="branch-name">{branch.branchName}</span>
      </div>
      <div class="header-actions">
        <div
          class="status-badge"
          class:starting={status === 'starting'}
          class:running={status === 'running'}
          class:stopped={status === 'stopped'}
          class:error={status === 'error'}
        >
          {#if status === 'starting'}
            <Loader2 size={12} class="spinner" />
          {:else if status === 'running'}
            <CircleCheck size={12} />
          {:else if status === 'stopped'}
            <CirclePause size={12} />
          {:else if status === 'error'}
            <AlertCircle size={12} />
          {/if}
          <span>{statusLabel(status)}</span>
        </div>
        <DropdownMenu items={menuItems} />
      </div>
    </div>

    <!-- Subheader: agent + workspace info -->
    <div class="card-subheader">
      <div class="agent-badge">
        <Bot size={12} />
        <span>{agentLabel(branch.agent)}</span>
      </div>
      {#if branch.workspaceName}
        <span class="workspace-name">{branch.workspaceName}</span>
      {/if}
    </div>

    <!-- Content area — varies by status -->
    <div class="card-content">
      {#if status === 'starting'}
        <div class="status-view starting-view">
          <Loader2 size={20} class="spinner" />
          <span class="status-text">Provisioning workspace…</span>
          <span class="status-hint">This usually takes 30–60 seconds</span>
        </div>
      {:else if status === 'running'}
        <!-- Conversation history -->
        {#if messages.length > 0}
          <div class="messages" bind:this={messagesEl}>
            {#each messages as msg}
              <div
                class="message"
                class:user={msg.role === 'user'}
                class:assistant={msg.role === 'assistant'}
              >
                <div class="message-role">
                  {msg.role === 'user' ? 'You' : agentLabel(branch.agent)}
                </div>
                <div class="message-content">{msg.content}</div>
              </div>
            {/each}
            {#if sending}
              <div class="message assistant">
                <div class="message-role">{agentLabel(branch.agent)}</div>
                <div class="message-content thinking">
                  <Loader2 size={14} class="spinner" />
                  <span>Thinking…</span>
                </div>
              </div>
            {/if}
          </div>
        {:else}
          <div class="empty-state">
            <span class="empty-text">Workspace is ready. Send a prompt to start working.</span>
          </div>
        {/if}

        {#if error}
          <div class="error-banner">
            <AlertCircle size={14} />
            <span>{error}</span>
          </div>
        {/if}

        <!-- Prompt input -->
        <div class="prompt-bar">
          <textarea
            class="prompt-input"
            bind:value={promptText}
            onkeydown={handleKeydown}
            placeholder="Send a task to {agentLabel(branch.agent)}…"
            rows={1}
            disabled={sending}
          ></textarea>
          <button
            class="send-btn"
            onclick={handleSendPrompt}
            disabled={!promptText.trim() || sending}
            title="Send prompt"
          >
            {#if sending}
              <Loader2 size={16} class="spinner" />
            {:else}
              <Send size={16} />
            {/if}
          </button>
        </div>
      {:else if status === 'stopped'}
        <div class="status-view stopped-view">
          <CirclePause size={20} />
          <span class="status-text">Workspace stopped</span>
          <span class="status-hint">Delete and recreate to start a new workspace</span>
        </div>
      {:else if status === 'error'}
        <div class="status-view error-view">
          <AlertCircle size={20} />
          <span class="status-text">Workspace error</span>
          {#if error}
            <span class="status-hint">{error}</span>
          {:else}
            <span class="status-hint">Something went wrong. Try deleting and recreating.</span>
          {/if}
        </div>
      {:else}
        <div class="status-view">
          <span class="status-text">Unknown status</span>
        </div>
      {/if}
    </div>
  {/if}
</div>

{#if confirmDelete}
  <ConfirmDialog
    title={confirmDelete.title}
    message={confirmDelete.message}
    confirmLabel="Delete"
    danger
    onConfirm={confirmDelete.onConfirm}
    onCancel={() => (confirmDelete = null)}
  />
{/if}

<style>
  .branch-card {
    display: flex;
    flex-direction: column;
    background-color: var(--bg-primary);
    border-radius: 8px;
    border: 1px solid var(--border-subtle);
    transition: border-color 0.15s ease;
  }

  .branch-card:hover:not(.deleting) {
    border-color: var(--border-muted);
  }

  .branch-card.deleting {
    opacity: 0.6;
  }

  /* Deleting overlay */
  .deleting-overlay {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 20px 16px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  /* Header */
  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px 0;
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  :global(.cloud-icon) {
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  .branch-name {
    font-size: var(--size-md);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  /* Status badge */
  .status-badge {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 3px 10px;
    border-radius: 12px;
    font-size: var(--size-xs);
    font-weight: 500;
    white-space: nowrap;
  }

  .status-badge.starting {
    background-color: rgba(210, 153, 34, 0.1);
    color: rgb(210, 153, 34);
  }

  .status-badge.running {
    background-color: rgba(63, 185, 80, 0.1);
    color: var(--ui-accent);
  }

  .status-badge.stopped {
    background-color: rgba(139, 148, 158, 0.1);
    color: var(--text-muted);
  }

  .status-badge.error {
    background-color: rgba(248, 81, 73, 0.1);
    color: var(--ui-danger);
  }

  /* Subheader */
  .card-subheader {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px 12px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .agent-badge {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: var(--size-xs);
    font-weight: 500;
    color: var(--text-muted);
  }

  .agent-badge :global(svg) {
    color: var(--text-faint);
  }

  .workspace-name {
    font-size: var(--size-xs);
    color: var(--text-faint);
    font-family: 'SF Mono', 'Menlo', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Content */
  .card-content {
    display: flex;
    flex-direction: column;
    min-height: 80px;
  }

  /* Status views (starting, stopped, error) */
  .status-view {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 32px 16px;
    text-align: center;
  }

  .status-view :global(svg) {
    color: var(--text-faint);
  }

  .starting-view :global(svg) {
    color: rgb(210, 153, 34);
  }

  .error-view :global(svg) {
    color: var(--ui-danger);
  }

  .status-text {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .status-hint {
    font-size: var(--size-xs);
    color: var(--text-muted);
    max-width: 280px;
  }

  /* Messages */
  .messages {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    max-height: 300px;
    overflow-y: auto;
  }

  .message {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .message-role {
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .message-content {
    font-size: var(--size-sm);
    color: var(--text-primary);
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .message.user .message-content {
    background-color: var(--bg-hover);
    padding: 8px 12px;
    border-radius: 8px;
  }

  .message.assistant .message-content {
    padding: 4px 0;
  }

  .message-content.thinking {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-style: italic;
  }

  /* Empty state */
  .empty-state {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px 16px;
  }

  .empty-text {
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  /* Error banner */
  .error-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 16px 8px;
    padding: 8px 12px;
    background-color: rgba(248, 81, 73, 0.08);
    border-radius: 6px;
    font-size: var(--size-xs);
    color: var(--ui-danger);
  }

  .error-banner :global(svg) {
    flex-shrink: 0;
  }

  /* Prompt bar */
  .prompt-bar {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--border-subtle);
  }

  .prompt-input {
    flex: 1;
    padding: 8px 12px;
    background-color: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    font-size: var(--size-sm);
    font-family: inherit;
    color: var(--text-primary);
    outline: none;
    resize: none;
    min-height: 36px;
    max-height: 120px;
    line-height: 1.4;
    transition: border-color 0.15s;
  }

  .prompt-input:focus {
    border-color: var(--ui-accent);
  }

  .prompt-input::placeholder {
    color: var(--text-faint);
  }

  .prompt-input:disabled {
    opacity: 0.5;
  }

  .send-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 36px;
    height: 36px;
    padding: 0;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 8px;
    color: var(--bg-deepest);
    cursor: pointer;
    flex-shrink: 0;
    transition: background-color 0.15s;
  }

  .send-btn:hover:not(:disabled) {
    background-color: var(--ui-accent-hover);
  }

  .send-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  :global(.spinner) {
    animation: spin 1s linear infinite;
    flex-shrink: 0;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
