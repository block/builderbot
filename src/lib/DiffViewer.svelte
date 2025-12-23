<script lang="ts">
  import type { FileDiff, DiffLine } from './types';

  interface Props {
    diff: FileDiff | null;
    onStageFile?: () => void;
    onDiscardFile?: () => void;
  }

  let { diff, onStageFile, onDiscardFile }: Props = $props();

  let leftPane: HTMLDivElement | null = $state(null);
  let rightPane: HTMLDivElement | null = $state(null);
  let isSyncing = false;

  function getLineClass(type: string): string {
    switch (type) {
      case 'added': return 'line-added';
      case 'removed': return 'line-removed';
      case 'empty': return 'line-empty';
      default: return 'line-context';
    }
  }

  function formatLineNumber(num: number | null): string {
    return num !== null ? String(num) : '';
  }

  function syncScroll(source: HTMLDivElement, target: HTMLDivElement | null) {
    if (isSyncing || !target) return;
    isSyncing = true;
    target.scrollTop = source.scrollTop;
    target.scrollLeft = source.scrollLeft;
    requestAnimationFrame(() => {
      isSyncing = false;
    });
  }

  function handleLeftScroll(e: Event) {
    const target = e.target as HTMLDivElement;
    syncScroll(target, rightPane);
  }

  function handleRightScroll(e: Event) {
    const target = e.target as HTMLDivElement;
    syncScroll(target, leftPane);
  }
</script>

<div class="diff-viewer">
  {#if diff === null}
    <div class="empty-state">
      <p>Select a file to view changes</p>
    </div>
  {:else if diff.is_binary}
    <div class="diff-header">
      <span class="file-path">{diff.path}</span>
      <div class="diff-actions">
        {#if onStageFile}
          <button class="action-btn" onclick={onStageFile} title="Stage file">Stage</button>
        {/if}
        {#if onDiscardFile}
          <button class="action-btn danger" onclick={onDiscardFile} title="Discard changes">Discard</button>
        {/if}
      </div>
    </div>
    <div class="binary-notice">
      <p>Binary file - cannot display diff</p>
    </div>
  {:else}
    <div class="diff-header">
      <span class="file-path">
        {#if diff.old_path}
          <span class="old-path">{diff.old_path}</span>
          <span class="arrow">→</span>
        {/if}
        {diff.path}
      </span>
      <div class="diff-actions">
        {#if onStageFile}
          <button class="action-btn" onclick={onStageFile} title="Stage file">Stage</button>
        {/if}
        {#if onDiscardFile}
          <button class="action-btn danger" onclick={onDiscardFile} title="Discard changes">Discard</button>
        {/if}
      </div>
    </div>

    <div class="diff-content">
      <div class="diff-pane left-pane">
        <div class="pane-header">Original</div>
        <div class="code-container" bind:this={leftPane} onscroll={handleLeftScroll}>
          {#each diff.old_content as line}
            <div class="line {getLineClass(line.line_type)}">
              <span class="line-number">{formatLineNumber(line.old_lineno)}</span>
              <span class="line-content">{line.content}</span>
            </div>
          {/each}
          {#if diff.old_content.length === 0}
            <div class="empty-file-notice">New file</div>
          {/if}
        </div>
      </div>

      <div class="diff-pane right-pane">
        <div class="pane-header">Modified</div>
        <div class="code-container" bind:this={rightPane} onscroll={handleRightScroll}>
          {#each diff.new_content as line}
            <div class="line {getLineClass(line.line_type)}">
              <span class="line-number">{formatLineNumber(line.new_lineno)}</span>
              <span class="line-content">{line.content}</span>
            </div>
          {/each}
          {#if diff.new_content.length === 0}
            <div class="empty-file-notice">File deleted</div>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .diff-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .empty-state, .binary-notice {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: #888;
    font-size: 14px;
  }

  .diff-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 16px;
    background-color: #2d2d2d;
    border-bottom: 1px solid #3c3c3c;
  }

  .file-path {
    font-family: monospace;
    font-size: 13px;
    color: #e2c08d;
  }

  .old-path {
    color: #888;
    text-decoration: line-through;
  }

  .arrow {
    margin: 0 8px;
    color: #888;
  }

  .diff-actions {
    display: flex;
    gap: 8px;
  }

  .action-btn {
    padding: 4px 12px;
    font-size: 12px;
    background-color: #0e639c;
    color: white;
    border: none;
    border-radius: 3px;
    cursor: pointer;
  }

  .action-btn:hover {
    background-color: #1177bb;
  }

  .action-btn.danger {
    background-color: #5a1d1d;
  }

  .action-btn.danger:hover {
    background-color: #742a2a;
  }

  .diff-content {
    display: flex;
    flex: 1;
    overflow: hidden;
  }

  .diff-pane {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .left-pane {
    border-right: 1px solid #3c3c3c;
  }

  .pane-header {
    padding: 6px 12px;
    font-size: 11px;
    text-transform: uppercase;
    color: #888;
    background-color: #2d2d2d;
    border-bottom: 1px solid #3c3c3c;
  }

  .code-container {
    flex: 1;
    overflow: auto;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: 13px;
    line-height: 1.5;
  }

  .empty-file-notice {
    padding: 20px;
    color: #888;
    font-style: italic;
  }

  .line {
    display: flex;
    min-height: 20px;
  }

  .line-number {
    width: 50px;
    padding: 0 12px;
    text-align: right;
    color: #6e7681;
    background-color: #1e1e1e;
    user-select: none;
    flex-shrink: 0;
  }

  .line-content {
    flex: 1;
    padding: 0 12px;
    white-space: pre;
  }

  .line-context {
    background-color: #1e1e1e;
  }

  .line-context .line-content {
    background-color: #1e1e1e;
  }

  .line-added {
    background-color: #2ea04326;
  }

  .line-added .line-number {
    background-color: #2ea04326;
    color: #7ee787;
  }

  .line-added .line-content {
    background-color: #2ea04326;
  }

  .line-removed {
    background-color: #f8514926;
  }

  .line-removed .line-number {
    background-color: #f8514926;
    color: #f85149;
  }

  .line-removed .line-content {
    background-color: #f8514926;
  }

  .line-empty {
    background-color: #2d2d2d;
  }

  .line-empty .line-number {
    background-color: #2d2d2d;
  }

  .line-empty .line-content {
    background-color: #2d2d2d;
  }
</style>
