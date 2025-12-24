<script lang="ts">
  import type { FileDiff } from './types';

  interface Props {
    diff: FileDiff | null;
  }

  let { diff }: Props = $props();

  let leftPane: HTMLDivElement | null = $state(null);
  let rightPane: HTMLDivElement | null = $state(null);
  let isSyncing = false;

  function getLineClass(type: string): string {
    switch (type) {
      case 'added':
        return 'line-added';
      case 'removed':
        return 'line-removed';
      case 'empty':
        return 'line-empty';
      default:
        return 'line-context';
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

  .empty-state,
  .binary-notice {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-muted);
    font-size: 14px;
  }

  .diff-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 16px;
    background-color: var(--diff-header-bg);
    border-bottom: 1px solid var(--border-primary);
  }

  .file-path {
    font-family: monospace;
    font-size: 13px;
    color: var(--status-modified);
  }

  .old-path {
    color: var(--text-muted);
    text-decoration: line-through;
  }

  .arrow {
    margin: 0 8px;
    color: var(--text-muted);
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
    border-right: 1px solid var(--border-primary);
  }

  .pane-header {
    padding: 6px 12px;
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-muted);
    background-color: var(--diff-header-bg);
    border-bottom: 1px solid var(--border-primary);
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
    color: var(--text-muted);
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
    color: var(--diff-line-number);
    background-color: var(--diff-context-bg);
    user-select: none;
    flex-shrink: 0;
  }

  .line-content {
    flex: 1;
    padding: 0 12px;
    white-space: pre;
  }

  .line-context {
    background-color: var(--diff-context-bg);
  }

  .line-context .line-content {
    background-color: var(--diff-context-bg);
  }

  .line-added {
    background-color: var(--diff-added-bg);
  }

  .line-added .line-number {
    background-color: var(--diff-added-bg);
    color: var(--diff-added-text);
  }

  .line-added .line-content {
    background-color: var(--diff-added-bg);
  }

  .line-removed {
    background-color: var(--diff-removed-bg);
  }

  .line-removed .line-number {
    background-color: var(--diff-removed-bg);
    color: var(--diff-removed-text);
  }

  .line-removed .line-content {
    background-color: var(--diff-removed-bg);
  }

  .line-empty {
    background-color: var(--diff-empty-bg);
  }

  .line-empty .line-number {
    background-color: var(--diff-empty-bg);
  }

  .line-empty .line-content {
    background-color: var(--diff-empty-bg);
  }
</style>
