<!--
  ActionOutputModal.svelte — View real-time output from a running action

  Displays stdout/stderr output from a running project action with ANSI color
  support. Shows status badges, allows stopping the action, and automatically
  scrolls to follow new output.

  Features:
  - Real-time output streaming with ANSI color rendering
  - Status badge (Running/Completed/Failed/Stopped)
  - Stop button when running
  - Automatically fetches buffered output on mount
  - Auto-scrolls to follow new output
  - Distinguishes stdout (normal) from stderr (dimmed)

  Props:
    executionId — the execution to display output for
    actionName  — name of the action being run
    onClose     — callback to close this modal
-->
<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { X, Loader2, AlertCircle, CircleStop, CheckCircle, XCircle } from 'lucide-svelte';
  import Convert from 'ansi-to-html';
  import { sanitize } from './sanitize';
  import type {
    ActionStatusEvent,
    ActionOutputEvent,
    OutputChunk,
    ActionStatus,
  } from './services/actions';
  import {
    getActionOutputBuffer,
    stopBranchAction,
    clearActionExecution,
    listenToActionOutput,
    listenToActionStatus,
  } from './services/actions';

  interface Props {
    executionId: string;
    actionName: string;
    onClose: () => void;
    onRemove?: (executionId: string) => void;
  }

  let { executionId, actionName, onClose, onRemove }: Props = $props();

  // =========================================================================
  // State
  // =========================================================================

  let status = $state<ActionStatus>('running');
  let exitCode = $state<number | null>(null);
  let outputChunks = $state<OutputChunk[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let stopping = $state(false);
  let outputEl: HTMLDivElement;
  let unlistenOutput: (() => void) | null = null;
  let unlistenStatus: (() => void) | null = null;
  let shouldAutoScroll = $state(true);

  // ANSI to HTML converter
  const ansiConverter = new Convert({
    fg: '#e0e0e0',
    bg: '#1e1e1e',
    newline: false,
    escapeXML: true,
    stream: false,
  });

  let isRunning = $derived(status === 'running');

  // =========================================================================
  // Lifecycle
  // =========================================================================

  onMount(async () => {
    await loadBufferedOutput();
    await setupListeners();
    // Scroll to bottom after initial load
    tick().then(() => scrollToBottom());
  });

  onDestroy(() => {
    cleanup();
  });

  // =========================================================================
  // Data loading
  // =========================================================================

  async function loadBufferedOutput() {
    try {
      loading = true;
      error = null;
      const buffer = await getActionOutputBuffer(executionId);
      if (buffer) {
        outputChunks = buffer;
      }
    } catch (e: any) {
      error = e?.message || 'Failed to load action output';
      console.error('Failed to load buffered output:', e);
    } finally {
      loading = false;
    }
  }

  async function setupListeners() {
    try {
      // Listen for output events
      unlistenOutput = await listenToActionOutput((event: ActionOutputEvent) => {
        if (event.executionId === executionId) {
          outputChunks = [
            ...outputChunks,
            {
              chunk: event.chunk,
              stream: event.stream,
              timestamp: Date.now(),
            },
          ];
          // Auto-scroll to bottom if user is already at bottom
          if (shouldAutoScroll) {
            tick().then(() => scrollToBottom());
          }
        }
      });

      // Listen for status changes
      unlistenStatus = await listenToActionStatus((event: ActionStatusEvent) => {
        if (event.executionId === executionId) {
          status = event.status;
          if (event.exitCode !== undefined) {
            exitCode = event.exitCode;
          }
        }
      });
    } catch (e: any) {
      console.error('Failed to setup event listeners:', e);
    }
  }

  function cleanup() {
    if (unlistenOutput) {
      unlistenOutput();
      unlistenOutput = null;
    }
    if (unlistenStatus) {
      unlistenStatus();
      unlistenStatus = null;
    }
  }

  // =========================================================================
  // Actions
  // =========================================================================

  async function handleStop() {
    if (stopping) return;
    stopping = true;
    try {
      await stopBranchAction(executionId);
      status = 'stopped';
    } catch (e: any) {
      error = e?.message || 'Failed to stop action';
      console.error('Failed to stop action:', e);
    } finally {
      stopping = false;
    }
  }

  function scrollToBottom() {
    if (outputEl) {
      outputEl.scrollTop = outputEl.scrollHeight;
    }
  }

  function handleScroll() {
    if (outputEl) {
      // Check if user is at the bottom (within 50px)
      const isAtBottom = outputEl.scrollHeight - outputEl.scrollTop - outputEl.clientHeight < 50;
      shouldAutoScroll = isAtBottom;
    }
  }

  function handleBackdropClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      onClose();
    }
  }

  async function handleRemove() {
    try {
      // Clear the execution from the backend
      await clearActionExecution(executionId);
      // Notify parent to remove from UI
      onRemove?.(executionId);
      // Close the modal
      onClose();
    } catch (e: any) {
      error = e?.message || 'Failed to remove execution';
      console.error('Failed to remove execution:', e);
    }
  }

  // =========================================================================
  // Rendering helpers
  // =========================================================================

  function renderChunk(chunk: OutputChunk): string {
    // Convert ANSI codes to HTML
    const html = ansiConverter.toHtml(chunk.chunk);
    // Sanitize the HTML to prevent XSS
    return sanitize(html);
  }

  function getStatusIcon(s: ActionStatus) {
    switch (s) {
      case 'running':
        return Loader2;
      case 'completed':
        return CheckCircle;
      case 'failed':
        return XCircle;
      case 'stopped':
        return CircleStop;
    }
  }

  function getStatusClass(s: ActionStatus): string {
    switch (s) {
      case 'running':
        return 'status-running';
      case 'completed':
        return 'status-completed';
      case 'failed':
        return 'status-failed';
      case 'stopped':
        return 'status-stopped';
    }
  }

  function getStatusLabel(s: ActionStatus): string {
    switch (s) {
      case 'running':
        return 'Running';
      case 'completed':
        return exitCode === 0 ? 'Completed' : `Completed (exit ${exitCode})`;
      case 'failed':
        return exitCode !== null ? `Failed (exit ${exitCode})` : 'Failed';
      case 'stopped':
        return 'Stopped';
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal" role="presentation" onclick={(e) => e.stopPropagation()}>
    <!-- Header -->
    <header class="modal-header">
      <div class="header-content">
        <span class="header-title">{actionName}</span>
        {#if status}
          {@const StatusIcon = getStatusIcon(status)}
          <div class="status-badge {getStatusClass(status)}">
            <StatusIcon size={12} class={status === 'running' ? 'spinning' : ''} />
            <span>{getStatusLabel(status)}</span>
          </div>
        {/if}
      </div>
      <div class="header-actions">
        {#if isRunning && !stopping}
          <button class="stop-btn" onclick={handleStop} title="Stop action">
            <CircleStop size={14} />
            <span>Stop</span>
          </button>
        {/if}
        {#if status === 'failed' && onRemove}
          <button class="remove-btn" onclick={handleRemove} title="Remove this failed run">
            <span>Remove</span>
          </button>
        {/if}
        <button class="close-btn" onclick={onClose} title="Close (Esc)">
          <X size={16} />
        </button>
      </div>
    </header>

    <!-- Output area -->
    <div class="modal-content" bind:this={outputEl} onscroll={handleScroll}>
      {#if loading}
        <div class="center-state">
          <Loader2 size={24} class="spinning" />
          <span>Loading output…</span>
        </div>
      {:else if error}
        <div class="center-state error">
          <AlertCircle size={24} />
          <span>{error}</span>
        </div>
      {:else if outputChunks.length === 0}
        <div class="center-state">
          <span>No output yet…</span>
        </div>
      {:else}
        <div class="output">
          {#each outputChunks as chunk}
            <div class="output-line {chunk.stream === 'stderr' ? 'stderr' : 'stdout'}">
              {@html renderChunk(chunk)}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    backdrop-filter: blur(2px);
  }

  .modal {
    background: var(--bg-primary);
    border-radius: 12px;
    width: 90vw;
    max-width: 1000px;
    height: 80vh;
    max-height: 800px;
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.3);
    border: 1px solid var(--border-primary);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-primary);
    background: var(--bg-secondary);
    border-radius: 12px 12px 0 0;
    flex-shrink: 0;
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 12px;
    flex: 1;
    min-width: 0;
  }

  .header-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .status-badge {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 12px;
    font-size: 12px;
    font-weight: 500;
    white-space: nowrap;
  }

  .status-running {
    background: rgba(59, 130, 246, 0.1);
    color: #3b82f6;
    border: 1px solid rgba(59, 130, 246, 0.2);
  }

  .status-completed {
    background: rgba(34, 197, 94, 0.1);
    color: #22c55e;
    border: 1px solid rgba(34, 197, 94, 0.2);
  }

  .status-failed {
    background: rgba(239, 68, 68, 0.1);
    color: #ef4444;
    border: 1px solid rgba(239, 68, 68, 0.2);
  }

  .status-stopped {
    background: rgba(156, 163, 175, 0.1);
    color: #9ca3af;
    border: 1px solid rgba(156, 163, 175, 0.2);
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .stop-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: rgba(239, 68, 68, 0.1);
    color: #ef4444;
    border: 1px solid rgba(239, 68, 68, 0.2);
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .stop-btn:hover {
    background: rgba(239, 68, 68, 0.15);
    border-color: rgba(239, 68, 68, 0.3);
  }

  .remove-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .remove-btn:hover {
    background: var(--bg-hover);
    border-color: var(--border-focus);
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    background: transparent;
    color: var(--text-secondary);
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: all 0.15s;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-content {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 16px;
    background: #1e1e1e;
  }

  .center-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    height: 100%;
    color: var(--text-secondary);
    font-size: 14px;
  }

  .center-state.error {
    color: #ef4444;
  }

  .output {
    font-family: 'SF Mono', 'Monaco', 'Menlo', 'Consolas', monospace;
    font-size: 13px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-wrap: break-word;
  }

  .output-line {
    margin: 0;
    padding: 0;
  }

  .output-line.stdout {
    color: #e0e0e0;
  }

  .output-line.stderr {
    color: #9ca3af;
  }

  :global(.spinning) {
    animation: spin 1s linear infinite;
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
