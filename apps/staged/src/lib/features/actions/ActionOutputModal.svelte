<!--
  ActionOutputModal.svelte — View real-time output from a running action

  Displays stdout/stderr output from a running project action with ANSI color
  support. Shows status badges, allows stopping the action, and automatically
  scrolls to follow new output.

  Features:
  - Real-time output streaming with ANSI color rendering
  - Status badge (Running/Completed/Failed/Stopped)
  - Stop button when running
  - Shows the output buffered so far, then follows the live output from
    exactly where that ends (see outputFollower)
  - Auto-scrolls to follow new output
  - Distinguishes stdout (normal) from stderr (dimmed)

  What it holds is bounded: the most recent lines only (see
  OUTPUT_LINE_LIMITS), with a count of the earlier ones. It holds nothing
  while closed, so the instance a branch card keeps mounted costs nothing
  between uses.

  Props:
    executionId — the execution to display output for
    branchId    — the branch saved notes attach to; omit for repo-scoped
                  executions, which hides the save-as-note affordance
    actionName  — name of the action being run
    onClose     — callback to close this modal
-->
<script lang="ts">
  import { onDestroy, tick, untrack } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import AlertCircle from '@lucide/svelte/icons/alert-circle';
  import CircleStop from '@lucide/svelte/icons/circle-stop';
  import CheckCircle from '@lucide/svelte/icons/check-circle';
  import XCircle from '@lucide/svelte/icons/x-circle';
  import Check from '@lucide/svelte/icons/check';
  import StickyNote from '@lucide/svelte/icons/sticky-note';
  import RotateCw from '@lucide/svelte/icons/rotate-cw';
  import Spinner from '../../shared/Spinner.svelte';
  import Convert from 'ansi-to-html';
  import { sanitize } from '../../shared/sanitize';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { createNote } from '../../commands';
  import type { ActionStatusEvent, ActionOutputEvent, ActionStatus } from './actions';
  import {
    getActionOutputBuffer,
    stopBranchAction,
    clearActionExecution,
    listenToActionOutput,
    listenToActionStatus,
  } from './actions';
  import { createIncrementalProcessor, type OutputLine } from './processOutput';
  import { createOutputFollower, type OutputFollower } from './outputFollower';
  import { viewport } from '../../shared/viewport.svelte';

  interface Props {
    open: boolean;
    executionId: string;
    branchId?: string;
    actionName: string;
    isStopping?: boolean;
    onClose: () => void;
    onRemove?: (executionId: string) => void;
    onNoteCreated?: () => void;
    onRunAgain?: () => void;
  }

  let {
    open,
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

  /** The lines on show, and how many earlier ones have been dropped. */
  interface OutputView {
    lines: OutputLine[];
    droppedLines: number;
  }

  const EMPTY_VIEW: OutputView = { lines: [], droppedLines: 0 };

  let status = $state<ActionStatus>('running');
  let exitCode = $state<number | null>(null);
  // Turns output into lines, and holds them. Output is fed to it as it
  // arrives, so what waits for the next frame is bounded by its limits.
  let lineProcessor = createIncrementalProcessor();
  // What the output area renders. `lines` is the processor's own array, which
  // it updates in place: publishing means handing Svelte a new wrapper around
  // the same array, so no line is copied, and the keyed {#each} touches only
  // the rows whose line object changed.
  let view = $state.raw<OutputView>(EMPTY_VIEW);
  let loading = $state(true);
  // The execution has no buffered output: it is unknown, or its output was
  // cleared or evicted.
  let unavailable = $state(false);
  let error = $state<string | null>(null);
  let saveError = $state<string | null>(null);
  let stoppingExecutions = $state<Set<string>>(new Set());
  let outputEl: HTMLDivElement;
  let follower: OutputFollower | null = null;
  let unlistenOutput: (() => void) | null = null;
  let unlistenStatus: (() => void) | null = null;
  let shouldAutoScroll = $state(true);

  // Live output is published at most once per frame.
  let publishRaf: number | null = null;

  /** Show the processor's lines as they now stand. */
  function publishLines() {
    if (publishRaf !== null) {
      cancelAnimationFrame(publishRaf);
      publishRaf = null;
    }
    view = { lines: lineProcessor.lines, droppedLines: lineProcessor.droppedLines };
    if (shouldAutoScroll) {
      tick().then(() => scrollToBottom());
    }
  }

  function schedulePublish() {
    if (publishRaf === null) {
      publishRaf = requestAnimationFrame(() => {
        publishRaf = null;
        publishLines();
      });
    }
  }

  /** Let go of every line. */
  function releaseLines() {
    if (publishRaf !== null) {
      cancelAnimationFrame(publishRaf);
      publishRaf = null;
    }
    lineProcessor = createIncrementalProcessor();
    view = EMPTY_VIEW;
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

  /** Get plain text from all the output lines held. */
  function getFullOutputText(): string {
    return lineProcessor.lines.map((l) => l.text).join('\n');
  }

  async function handleSaveAsNote() {
    if (!branchId || saveState === 'saved') return;
    const content = capturedSelection || selectedText || getFullOutputText();
    capturedSelection = '';
    if (!content) return;
    try {
      saveError = null;
      const title = `${actionName} log`;
      await createNote(branchId, title, '```\n' + content + '\n```');
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

  // =========================================================================
  // Lifecycle
  // =========================================================================

  // Only track the selection while the modal is open. Every card mounts one of
  // these persistently, so an unconditional document listener would cost a
  // handler per mounted card on every selection change anywhere in the app,
  // each walking an unmounted element's subtree.
  $effect(() => {
    if (!open) return;
    document.addEventListener('selectionchange', handleSelectionChange);
    return () => {
      document.removeEventListener('selectionchange', handleSelectionChange);
      selectedText = '';
    };
  });

  onDestroy(() => {
    if (saveTimeout) clearTimeout(saveTimeout);
    stopFollowing();
  });

  // React to executionId changes (e.g. when "Run again" switches to a new execution)
  $effect(() => {
    // Bail out unless the modal is actually open with a real execution. The
    // modal is mounted persistently (open is a prop), so without this guard
    // every closed instance would still fetch getActionOutputBuffer('') and
    // install output/status listeners — wasteful on pages with many cards,
    // and it surfaces a spurious "not found" load error.
    const following = open ? executionId : '';
    untrack(() => {
      stopFollowing();
      // A closing modal keeps its lines until it has gone from the screen;
      // see handleOpenChangeComplete.
      if (!following) return;

      // Reset state for the new execution
      status = 'running';
      exitCode = null;
      releaseLines();
      loading = true;
      unavailable = false;
      error = null;
      shouldAutoScroll = true;
      startFollowing(following);
    });
  });

  // The lines stay while the dialog animates out, so its last frames still
  // show them, and are released once it is gone. A branch card keeps this
  // component mounted while closed; without this the last output it showed
  // would stay in memory until the next one was opened.
  function handleOpenChangeComplete(isOpen: boolean) {
    if (!isOpen) releaseLines();
  }

  // =========================================================================
  // Data loading
  // =========================================================================

  /**
   * Show `id`'s output: what is buffered, then what is written from there on.
   *
   * The listener is registered first and the buffer is read once it is live.
   * Output written before that is then in the buffer, and output written
   * after it arrives as an event; the follower uses the offsets both carry to
   * show each chunk exactly once. Reading the buffer first, as this used to,
   * lost whatever was written before the listener went live.
   */
  function startFollowing(id: string) {
    const current = createOutputFollower({
      fetchSnapshot: () => getActionOutputBuffer(id),
      onSnapshot: (chunks) => {
        // A snapshot replaces whatever was shown. After the first it is the
        // recovery from lost events, which starts the lines over.
        lineProcessor = createIncrementalProcessor();
        lineProcessor.process(chunks);
        loading = false;
        unavailable = false;
        error = null;
        publishLines();
      },
      onOutput: (output) => {
        lineProcessor.process([output]);
        schedulePublish();
      },
      onUnavailable: () => {
        loading = false;
        unavailable = true;
      },
      onError: (e: any) => {
        console.error('Failed to load buffered output:', e);
        // Only the first load has nothing to fall back on. A later one keeps
        // what is on show, and the follower tries again.
        if (loading) {
          loading = false;
          error = e?.message || 'Failed to load action output';
        }
      },
    });
    follower = current;

    unlistenOutput = listenToActionOutput(
      (event: ActionOutputEvent) => {
        if (event.executionId === id) current.handleEvent(event);
      },
      {
        // Fires when the listener is live, and again after a reconnect or an
        // event gap, when events may have been lost.
        onEstablished: () => current.sync(),
        // No live output will arrive. Show what is buffered all the same.
        onRegistrationFailed: () => current.sync(),
      }
    );

    // Listen for status changes
    unlistenStatus = listenToActionStatus((event: ActionStatusEvent) => {
      if (event.executionId === id) {
        status = event.status;
        if (event.exitCode !== undefined) {
          exitCode = event.exitCode;
        }
        // Clean up stopping state when action reaches terminal state
        if (status !== 'running') {
          const updated = new Set(stoppingExecutions);
          updated.delete(id);
          stoppingExecutions = updated;
        }
      }
    });
  }

  /** Stop listening and fetching. The lines on show are left as they are. */
  function stopFollowing() {
    if (publishRaf !== null) {
      cancelAnimationFrame(publishRaf);
      publishRaf = null;
    }
    follower?.stop();
    follower = null;
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

  // Nothing rendered is cached. The keyed {#each} calls this once for a line
  // object and not again while that object stays in the list, and a line whose
  // text changes is a new object (see processOutput), so there is nothing a
  // cache would save: the only copy of a line's HTML is its row.
  function renderLine(line: OutputLine): string {
    // Convert ANSI codes to HTML, then sanitize to prevent XSS
    return sanitize(ansiConverter.toHtml(line.text));
  }

  function droppedLinesLabel(count: number): string {
    return `${count.toLocaleString()} earlier ${count === 1 ? 'line' : 'lines'} not shown`;
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

<Dialog.Root
  {open}
  onOpenChange={(v) => !v && onClose()}
  onOpenChangeComplete={handleOpenChangeComplete}
>
  <Dialog.Content
    class="sm:max-w-[1000px] w-[90vw] h-[80vh] max-h-[800px] bg-background p-0 gap-0 overflow-hidden flex flex-col border border-[var(--border-primary)]"
    showCloseButton={false}
  >
    <!-- Header -->
    <header class="modal-header">
      <div class="header-content">
        <Dialog.Title
          class="text-[var(--size-sm)] font-semibold text-foreground overflow-hidden text-ellipsis whitespace-nowrap"
          >{actionName}</Dialog.Title
        >
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
        {#if branchId}
          <span
            class="inline-flex"
            title={saveState === 'error'
              ? (saveError ?? 'Failed to save note')
              : selectedText
                ? 'Save selected text as a note'
                : 'Save full log as a note'}
          >
            <Button
              variant="outline"
              size="sm"
              class={[
                'gap-1.5 border-[var(--border-muted)] bg-[var(--bg-secondary)] px-3 text-[13px] font-medium text-foreground shadow-none hover:border-[var(--border-focus)] hover:bg-[var(--bg-hover)] mobile:h-10 mobile:px-2.5',
                saveState === 'saved' &&
                  'cursor-default border-[var(--commit-bg-emphasis)] bg-[var(--commit-bg)] text-[var(--status-added)] hover:border-[var(--commit-bg-emphasis)] hover:bg-[var(--commit-bg)] hover:text-[var(--status-added)]',
                saveState === 'error' &&
                  'cursor-default border-[var(--ui-danger-bg)] bg-[var(--ui-danger-bg)] text-[var(--ui-danger)] hover:border-[var(--ui-danger-bg)] hover:bg-[var(--ui-danger-bg)] hover:text-[var(--ui-danger)]',
              ]}
              onmousedown={handleSaveMouseDown}
              onclick={handleSaveAsNote}
              disabled={saveState === 'saved' || saveState === 'error'}
            >
              {#if saveState === 'saved'}
                <Check size={14} />
                <span>Saved</span>
              {:else if saveState === 'error'}
                <AlertCircle size={14} />
                <span>Failed</span>
              {:else}
                <StickyNote size={14} />
                <span>{selectedText ? 'Save selection' : 'Save log'}</span>
              {/if}
            </Button>
          </span>
        {/if}
        {#if isRunning}
          {@const isCurrentlyStopping = isStopping || isStoppingDerived}
          <span class="inline-flex">
            <Button
              variant="destructive"
              size="sm"
              class="gap-1.5 px-3 text-[13px] font-medium mobile:h-10 mobile:px-2.5"
              onclick={handleStop}
              disabled={isCurrentlyStopping}
            >
              <CircleStop size={14} />
              <span>{isCurrentlyStopping ? 'Stopping…' : 'Stop'}</span>
            </Button>
          </span>
        {:else if onRunAgain}
          <Button
            variant="outline"
            size="sm"
            class="gap-1.5 border-[var(--border-muted)] bg-[var(--bg-secondary)] px-3 text-[13px] font-medium hover:border-[var(--border-focus)] hover:bg-[var(--bg-hover)] mobile:h-10 mobile:px-2.5"
            onclick={onRunAgain}
          >
            <RotateCw size={14} />
            <span>Run again</span>
          </Button>
        {/if}
        {#if status === 'failed' && onRemove}
          <Button
            variant="destructive"
            size="sm"
            class="gap-1.5 px-3 text-[13px] font-medium mobile:h-10 mobile:px-2.5"
            title="Remove this failed run"
            onclick={handleRemove}
          >
            <span>Remove</span>
          </Button>
        {/if}
        <Button
          variant="ghost"
          size="icon-sm"
          class="size-8 shrink-0 rounded-md text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-foreground mobile:size-10 [&_svg]:!size-4"
          title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
          aria-label="Close"
          onclick={onClose}
        >
          <X size={16} />
        </Button>
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
      {:else if unavailable}
        <div class="center-state">
          <span>Output is no longer available.</span>
        </div>
      {:else if view.lines.length === 0}
        <div class="center-state">
          <span>No output yet…</span>
        </div>
      {:else}
        <div class="output">
          {#if view.droppedLines > 0}
            <div class="output-line stderr dropped-lines">
              {droppedLinesLabel(view.droppedLines)}
            </div>
          {/if}
          {#each view.lines as line (line.id)}
            <div class="output-line {line.stream === 'stderr' ? 'stderr' : 'stdout'}">
              {@html renderLine(line)}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </Dialog.Content>
</Dialog.Root>

<style>
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

  /* Ends the dialog: in a full-screen one the output scrolls behind the home
     indicator and its last line can still scroll clear of it
     (`--dialog-safe-bottom`, see dialog-content). */
  .modal-content {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
    padding: 16px;
    padding-bottom: calc(16px + var(--dialog-safe-bottom, 0px));
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

  .output-line.dropped-lines {
    font-style: italic;
  }

  /* The mobile layout (MOBILE_LAYOUT_QUERY, viewport.svelte.ts). Rows span
     the full-screen dialog and clear its insets (`--dialog-safe-*`, see
     dialog-content), the header the top one too. */
  @media (max-width: 768px),
    (pointer: coarse) and (max-device-width: 500px),
    (pointer: coarse) and (max-device-height: 500px) {
    .modal-header {
      border-radius: 0;
      padding: 12px;
      padding-top: calc(12px + var(--dialog-safe-top, 0px));
      padding-left: calc(12px + var(--dialog-safe-left, 0px));
      padding-right: calc(12px + var(--dialog-safe-right, 0px));
    }

    .header-actions {
      gap: 4px;
    }

    .modal-content {
      padding: 12px;
      padding-bottom: calc(12px + var(--dialog-safe-bottom, 0px));
      padding-left: calc(12px + var(--dialog-safe-left, 0px));
      padding-right: calc(12px + var(--dialog-safe-right, 0px));
    }
  }
</style>
