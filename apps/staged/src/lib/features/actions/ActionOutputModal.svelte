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
  import {
    X,
    AlertCircle,
    CircleStop,
    CheckCircle,
    XCircle,
    Check,
    StickyNote,
    RotateCw,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import Convert from 'ansi-to-html';
  import { sanitize } from '../../shared/sanitize';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';
  import { createNote, invalidateBranchTimeline } from '../../commands';
  import type { ActionStatusEvent, ActionOutputEvent, OutputChunk, ActionStatus } from './actions';
  import {
    getActionOutputBuffer,
    stopBranchAction,
    clearActionExecution,
    listenToActionOutput,
    listenToActionStatus,
  } from './actions';
  import { createIncrementalProcessor, type TerminalLine } from './processOutput';
  import { viewport } from '../../shared/viewport.svelte';

  interface Props {
    executionId: string;
    branchId: string;
    actionName: string;
    isStopping?: boolean;
    onClose: () => void;
    onRemove?: (executionId: string) => void;
    onNoteCreated?: () => void;
    onRunAgain?: () => void;
  }

  let {
    executionId,
    branchId,
    actionName,
    isStopping = false,
    onClose,
    onRemove,
    onNoteCreated,
    onRunAgain,
  }: Props = $props();

  // =========================================================================
  // State
  // =========================================================================

  let status = $state<ActionStatus>('running');
  let exitCode = $state<number | null>(null);
  let displayLines = $state<TerminalLine[]>([]);
  let lineProcessor = createIncrementalProcessor();
  let loading = $state(true);
  let error = $state<string | null>(null);
  let saveError = $state<string | null>(null);
  let stoppingExecutions = $state<Set<string>>(new Set());
  let outputEl: HTMLDivElement;
  let unlistenOutput: (() => void) | null = null;
  let unlistenStatus: (() => void) | null = null;
  let shouldAutoScroll = $state(true);
  const backdropDismiss = createBackdropDismissHandlers({ onDismiss: () => onClose() });

  // rAF batching for incoming output chunks
  let pendingChunks: OutputChunk[] = [];
  let flushRaf: number | null = null;

  function flushPendingChunks() {
    flushRaf = null;
    if (pendingChunks.length > 0) {
      const chunks = pendingChunks;
      pendingChunks = [];
      displayLines = lineProcessor.process(chunks);
      if (shouldAutoScroll) {
        tick().then(() => scrollToBottom());
      }
    }
  }

  // ANSI to HTML converter
  const ansiConverter = new Convert({
    fg: '#e0e0e0',
    bg: '#1e1e1e',
    newline: false,
    escapeXML: true,
    stream: false,
  });

  let isRunning = $derived(status === 'running');
  let isStoppingDerived = $derived(stoppingExecutions.has(executionId));

  // Save-as-note state
  let selectedText = $state('');
  let capturedSelection = '';
  let saveState = $state<'idle' | 'saved' | 'error'>('idle');
  let saveTimeout: ReturnType<typeof setTimeout> | null = null;

  function handleSelectionChange() {
    const sel = document.getSelection();
    if (sel && outputEl?.contains(sel.anchorNode) && outputEl?.contains(sel.focusNode)) {
      selectedText = sel.toString().trim();
    } else {
      selectedText = '';
    }
  }

  /** Capture selection on mousedown before the click clears it. */
  function handleSaveMouseDown() {
    capturedSelection = selectedText;
  }

  /** Get plain text from all output lines. */
  function getFullOutputText(): string {
    return displayLines.map((l) => l.text).join('\n');
  }

  async function handleSaveAsNote() {
    if (saveState === 'saved') return;
    const content = capturedSelection || selectedText || getFullOutputText();
    capturedSelection = '';
    if (!content) return;
    try {
      saveError = null;
      const title = `${actionName} log`;
      await createNote(branchId, title, '```\n' + content + '\n```');
      invalidateBranchTimeline(branchId);
      onNoteCreated?.();
      saveState = 'saved';
      if (saveTimeout) clearTimeout(saveTimeout);
      saveTimeout = setTimeout(() => {
        saveState = 'idle';
      }, 2000);
    } catch (e: any) {
      saveError = e?.message || 'Failed to save note';
      console.error('Failed to save note:', e);
      saveState = 'error';
      if (saveTimeout) clearTimeout(saveTimeout);
      saveTimeout = setTimeout(() => {
        saveState = 'idle';
        saveError = null;
      }, 3000);
    }
  }

  // Render cache — avoids re-running ANSI conversion + sanitization for unchanged lines
  let renderLineCache = new Map<string, string>();

  // =========================================================================
  // Lifecycle
  // =========================================================================

  onMount(() => {
    document.addEventListener('selectionchange', handleSelectionChange);
  });

  onDestroy(() => {
    document.removeEventListener('selectionchange', handleSelectionChange);
    if (saveTimeout) clearTimeout(saveTimeout);
    cleanup();
  });

  // React to executionId changes (e.g. when "Run again" switches to a new execution)
  $effect(() => {
    void executionId; // subscribe to executionId changes
    // Reset state for the new execution
    status = 'running';
    exitCode = null;
    displayLines = [];
    lineProcessor = createIncrementalProcessor();
    renderLineCache = new Map();
    pendingChunks = [];
    if (flushRaf !== null) {
      cancelAnimationFrame(flushRaf);
      flushRaf = null;
    }
    loading = true;
    error = null;
    shouldAutoScroll = true;
    cleanup();

    (async () => {
      await loadBufferedOutput();
      await setupListeners();
      tick().then(() => scrollToBottom());
    })();
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
        displayLines = lineProcessor.process(buffer);
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
      // Listen for output events — batched via requestAnimationFrame
      unlistenOutput = await listenToActionOutput((event: ActionOutputEvent) => {
        if (event.executionId === executionId) {
          pendingChunks.push({
            chunk: event.chunk,
            stream: event.stream,
            timestamp: Date.now(),
          });
          if (flushRaf === null) {
            flushRaf = requestAnimationFrame(flushPendingChunks);
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
          // Clean up stopping state when action reaches terminal state
          if (status !== 'running') {
            const updated = new Set(stoppingExecutions);
            updated.delete(executionId);
            stoppingExecutions = updated;
          }
        }
      });
    } catch (e: any) {
      console.error('Failed to setup event listeners:', e);
    }
  }

  function cleanup() {
    if (flushRaf !== null) {
      cancelAnimationFrame(flushRaf);
      flushRaf = null;
    }
    pendingChunks = [];
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
    // Prevent duplicate stop requests
    if (isStoppingDerived) {
      return;
    }

    // Add to stopping set and trigger reactivity
    stoppingExecutions = new Set(stoppingExecutions).add(executionId);

    try {
      await stopBranchAction(executionId);
      // Backend will emit 'stopped' status event which will clean up the stopping state
    } catch (e: any) {
      // Remove from stopping set on error so user can retry
      const updated = new Set(stoppingExecutions);
      updated.delete(executionId);
      stoppingExecutions = updated;
      error = e?.message || 'Failed to stop action';
      console.error('Failed to stop action:', e);
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

  function renderLine(line: TerminalLine): string {
    const text = line.text;
    let cached = renderLineCache.get(text);
    if (cached !== undefined) return cached;
    // Convert ANSI codes to HTML, then sanitize to prevent XSS
    const html = ansiConverter.toHtml(text);
    cached = sanitize(html);
    renderLineCache.set(text, cached);
    return cached;
  }

  function getStatusIcon(s: ActionStatus) {
    switch (s) {
      case 'completed':
        return CheckCircle;
      case 'failed':
        return XCircle;
      case 'stopped':
        return CircleStop;
      default:
        return null;
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

  function getStatusLabel(s: ActionStatus, stopping: boolean): string {
    if (stopping && s === 'running') {
      return 'Stopping';
    }
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
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
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
          {@const isCurrentlyStopping = isStopping || isStoppingDerived}
          <div class="status-badge {getStatusClass(status)}">
            {#if status === 'running' && isCurrentlyStopping}
              <Spinner size={12} class="danger" />
            {:else if status === 'running'}
              <Spinner size={12} />
            {:else if StatusIcon}
              <StatusIcon size={12} />
            {/if}
            <span>{getStatusLabel(status, isCurrentlyStopping)}</span>
          </div>
        {/if}
      </div>
      <div class="header-actions">
        <button
          class="save-note-btn"
          class:saved={saveState === 'saved'}
          class:save-error={saveState === 'error'}
          onmousedown={handleSaveMouseDown}
          onclick={handleSaveAsNote}
          disabled={saveState === 'saved' || saveState === 'error'}
          title={saveState === 'error'
            ? (saveError ?? 'Failed to save note')
            : selectedText
              ? 'Save selected text as a note'
              : 'Save full log as a note'}
        >
          {#if saveState === 'saved'}
            <span class="save-note-label">
              <Check size={14} />
              <span>Saved</span>
            </span>
          {:else if saveState === 'error'}
            <span class="save-note-label">
              <AlertCircle size={14} />
              <span>Failed</span>
            </span>
          {:else}
            <span class="save-note-label">
              <StickyNote size={14} />
              <span>{selectedText ? 'Save selection' : 'Save log'}</span>
            </span>
          {/if}
        </button>
        {#if isRunning}
          {@const isCurrentlyStopping = isStopping || isStoppingDerived}
          <button
            class="stop-btn"
            onclick={handleStop}
            disabled={isCurrentlyStopping}
            title="Stop action"
          >
            <CircleStop size={14} />
            <span>{isCurrentlyStopping ? 'Stopping…' : 'Stop'}</span>
          </button>
        {:else if onRunAgain}
          <button class="run-again-btn" onclick={onRunAgain} title="Run again">
            <RotateCw size={14} />
            <span>Run again</span>
          </button>
        {/if}
        {#if status === 'failed' && onRemove}
          <button class="remove-btn" onclick={handleRemove} title="Remove this failed run">
            <span>Remove</span>
          </button>
        {/if}
        <button
          class="close-btn"
          onclick={onClose}
          title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
        >
          <X size={16} />
        </button>
      </div>
    </header>

    <!-- Output area -->
    <div class="modal-content" bind:this={outputEl} onscroll={handleScroll}>
      {#if loading}
        <div class="center-state">
          <Spinner size={24} />
          <span>Loading output…</span>
        </div>
      {:else if error}
        <div class="center-state error">
          <AlertCircle size={24} />
          <span>{error}</span>
        </div>
      {:else if displayLines.length === 0}
        <div class="center-state">
          <span>No output yet…</span>
        </div>
      {:else}
        <div class="output">
          {#each displayLines as line}
            <div class="output-line {line.stream === 'stderr' ? 'stderr' : 'stdout'}">
              {@html renderLine(line)}
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

  .stop-btn:hover:not(:disabled) {
    background: rgba(239, 68, 68, 0.15);
    border-color: rgba(239, 68, 68, 0.3);
  }

  .stop-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .run-again-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: rgba(59, 130, 246, 0.1);
    color: #3b82f6;
    border: 1px solid rgba(59, 130, 246, 0.2);
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .run-again-btn:hover {
    background: rgba(59, 130, 246, 0.15);
    border-color: rgba(59, 130, 246, 0.3);
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

  .save-note-btn {
    display: flex;
    align-items: center;
    padding: 6px 12px;
    background: var(--bg-secondary);
    color: var(--text-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: all 0.2s ease;
    overflow: hidden;
  }

  .save-note-btn:hover:not(:disabled) {
    background: var(--bg-hover);
    border-color: var(--border-focus);
  }

  .save-note-btn.saved {
    background: var(--commit-bg);
    color: var(--status-added);
    border-color: var(--commit-bg-emphasis);
    cursor: default;
  }

  .save-note-btn.save-error {
    background: var(--ui-danger-bg);
    color: var(--ui-danger);
    border-color: var(--ui-danger-bg);
    cursor: default;
  }

  .save-note-label {
    display: flex;
    align-items: center;
    gap: 6px;
    white-space: nowrap;
  }

  @media (max-width: 640px) {
    .modal {
      width: 100vw;
      max-width: none;
      height: 100vh;
      height: 100dvh;
      max-height: none;
      border-radius: 0;
      box-shadow: none;
    }

    .modal-header {
      border-radius: 0;
      padding: 12px;
    }

    .header-actions {
      gap: 4px;
    }

    .close-btn {
      width: 40px;
      height: 40px;
    }

    .stop-btn,
    .run-again-btn,
    .remove-btn,
    .save-note-btn {
      min-height: 40px;
      padding: 6px 10px;
    }

    .modal-content {
      padding: 12px;
    }
  }
</style>
