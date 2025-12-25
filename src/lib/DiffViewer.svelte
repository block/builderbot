<script lang="ts">
  import { onMount } from 'svelte';
  import type { FileDiff, DiffLine } from './types';
  import {
    initHighlighter,
    highlightLine,
    detectLanguage,
    prepareLanguage,
    getTheme,
    type Token,
  } from './services/highlighter';
  import { createScrollSync } from './services/scrollSync';

  interface Props {
    diff: FileDiff | null;
  }

  let { diff }: Props = $props();

  let beforePane: HTMLDivElement | null = $state(null);
  let afterPane: HTMLDivElement | null = $state(null);
  let highlighterReady = $state(false);
  let languageReady = $state(false);
  let themeBg = $state('#1e1e1e');

  // Scroll sync controller
  const scrollSync = createScrollSync();

  // Detect language from file path
  let language = $derived(
    diff?.after.path ? detectLanguage(diff.after.path) : 
    diff?.before.path ? detectLanguage(diff.before.path) : 
    null
  );

  // Update scroll sync ranges when diff changes
  $effect(() => {
    if (diff) {
      scrollSync.setRanges(diff.ranges);
    }
  });

  onMount(async () => {
    await initHighlighter('github-dark');
    const theme = getTheme();
    if (theme) {
      themeBg = theme.bg;
    }
    highlighterReady = true;
  });

  // Load language when file changes
  $effect(() => {
    if (highlighterReady && diff) {
      languageReady = false;
      const path = diff.after.path || diff.before.path;
      if (path) {
        prepareLanguage(path).then((ready) => {
          languageReady = ready;
        });
      }
    }
  });

  function getTokens(content: string): Token[] {
    if (!highlighterReady || !languageReady) {
      return [{ content, color: '#d4d4d4' }];
    }
    return highlightLine(content, language);
  }

  function handleBeforeScroll(e: Event) {
    if (!diff) return;
    const target = e.target as HTMLDivElement;
    scrollSync.onScroll('before', target, afterPane);
  }

  function handleAfterScroll(e: Event) {
    if (!diff) return;
    const target = e.target as HTMLDivElement;
    scrollSync.onScroll('after', target, beforePane);
  }

  // Get display path (handles renames)
  function getDisplayPath(): string {
    if (!diff) return '';
    const beforePath = diff.before.path;
    const afterPath = diff.after.path;
    
    if (beforePath && afterPath && beforePath !== afterPath) {
      return `${beforePath} → ${afterPath}`;
    }
    return afterPath || beforePath || '';
  }
</script>

<div class="diff-viewer">
  {#if diff === null}
    <div class="empty-state">
      <p>Select a file to view changes</p>
    </div>
  {:else if diff.is_binary}
    <div class="diff-header">
      <span class="file-path">{getDisplayPath()}</span>
    </div>
    <div class="binary-notice">
      <p>Binary file - cannot display diff</p>
    </div>
  {:else}
    <div class="diff-header">
      <span class="file-path">{getDisplayPath()}</span>
    </div>

    <div class="diff-content">
      <!-- Before pane -->
      <div class="diff-pane before-pane">
        <div class="pane-header">Before</div>
        <div
          class="code-container"
          bind:this={beforePane}
          onscroll={handleBeforeScroll}
          style="background-color: {themeBg}"
        >
          {#each diff.before.lines as line}
            <div class="line" class:line-removed={line.line_type === 'removed'}>
              <span class="line-number" class:gutter-removed={line.line_type === 'removed'}>
                {line.lineno}
              </span>
              <span class="line-content" class:content-removed={line.line_type === 'removed'}>
                {#each getTokens(line.content) as token}
                  <span style="color: {token.color}">{token.content}</span>
                {/each}
              </span>
            </div>
          {/each}
          {#if diff.before.lines.length === 0}
            <div class="empty-file-notice">New file</div>
          {/if}
        </div>
      </div>

      <!-- After pane -->
      <div class="diff-pane after-pane">
        <div class="pane-header">After</div>
        <div
          class="code-container"
          bind:this={afterPane}
          onscroll={handleAfterScroll}
          style="background-color: {themeBg}"
        >
          {#each diff.after.lines as line}
            <div class="line" class:line-added={line.line_type === 'added'}>
              <span class="line-number" class:gutter-added={line.line_type === 'added'}>
                {line.lineno}
              </span>
              <span class="line-content" class:content-added={line.line_type === 'added'}>
                {#each getTokens(line.content) as token}
                  <span style="color: {token.color}">{token.content}</span>
                {/each}
              </span>
            </div>
          {/each}
          {#if diff.after.lines.length === 0}
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

  .before-pane {
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

  /* Line number (gutter) styling */
  .line-number {
    width: 50px;
    padding: 0 12px;
    text-align: right;
    color: var(--diff-line-number);
    user-select: none;
    flex-shrink: 0;
  }

  .gutter-added {
    background-color: var(--diff-added-gutter);
    color: var(--diff-added-text);
  }

  .gutter-removed {
    background-color: var(--diff-removed-gutter);
    color: var(--diff-removed-text);
  }

  /* Line content styling */
  .line-content {
    flex: 1;
    padding: 0 12px;
    white-space: pre;
  }

  /* Overlay tints for diff highlighting */
  .content-added {
    background-color: var(--diff-added-overlay);
  }

  .content-removed {
    background-color: var(--diff-removed-overlay);
  }
</style>
