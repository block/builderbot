<script lang="ts">
  import { onMount } from 'svelte';
  import type { FileDiff } from './types';
  import {
    initHighlighter,
    highlightLine,
    detectLanguage,
    prepareLanguage,
    getTheme,
    type Token,
  } from './services/highlighter';
  import { createScrollSync } from './services/scrollSync';
  import { drawConnectors } from './diffConnectors';
  import { getDisplayPath, getLineBoundary, getLanguageFromDiff } from './diffUtils';
  import { setupKeyboardNav } from './diffKeyboard';

  interface Props {
    diff: FileDiff | null;
  }

  let { diff }: Props = $props();

  let beforePane: HTMLDivElement | null = $state(null);
  let afterPane: HTMLDivElement | null = $state(null);
  let connectorSvg: SVGSVGElement | null = $state(null);
  let highlighterReady = $state(false);
  let languageReady = $state(false);
  let themeBg = $state('#1e1e1e');

  const scrollSync = createScrollSync();

  $effect(() => {
    if (diff) {
      scrollSync.setRanges(diff.ranges);
    }
  });

  function handleBeforeScroll(e: Event) {
    if (!diff) return;
    const target = e.target as HTMLDivElement;
    scrollSync.onScroll('before', target, afterPane);
    redrawConnectors();
  }

  function handleAfterScroll(e: Event) {
    if (!diff) return;
    const target = e.target as HTMLDivElement;
    scrollSync.onScroll('after', target, beforePane);
    redrawConnectors();
  }

  let language = $derived(diff ? getLanguageFromDiff(diff, detectLanguage) : null);

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

  function redrawConnectors() {
    if (!connectorSvg || !beforePane || !afterPane || !diff) return;
    drawConnectors(connectorSvg, diff.ranges, beforePane.scrollTop, afterPane.scrollTop);
  }

  $effect(() => {
    if (diff && connectorSvg && beforePane) {
      const _ = beforePane.scrollTop; // dependency
      redrawConnectors();
    }
  });

  onMount(() => {
    initHighlighter('github-dark').then(() => {
      const theme = getTheme();
      if (theme) themeBg = theme.bg;
      highlighterReady = true;
    });

    return setupKeyboardNav({
      getScrollTarget: () => afterPane,
    });
  });
</script>

<div class="diff-viewer">
  {#if diff === null}
    <div class="empty-state">
      <p>Select a file to view changes</p>
    </div>
  {:else if diff.is_binary}
    <div class="diff-header">
      <span class="file-path">{getDisplayPath(diff)}</span>
    </div>
    <div class="binary-notice">
      <p>Binary file - cannot display diff</p>
    </div>
  {:else}
    <div class="diff-header">
      <span class="file-path">{getDisplayPath(diff)}</span>
    </div>

    <div class="diff-content">
      <!-- Before pane -->
      <div class="diff-pane">
        <div class="pane-header">Before</div>
        <div
          class="code-container"
          bind:this={beforePane}
          onscroll={handleBeforeScroll}
          style="background-color: {themeBg}"
        >
          {#each diff.before.lines as line, i}
            {@const boundary = getLineBoundary(diff.ranges, 'before', i)}
            <div
              class="line"
              class:line-removed={line.line_type === 'removed'}
              class:range-start={boundary.isStart}
              class:range-end={boundary.isEnd}
            >
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

      <!-- Range connectors -->
      <div class="connector-gutter">
        <div class="connector-header"></div>
        <svg class="connector-svg" bind:this={connectorSvg}></svg>
      </div>

      <!-- After pane -->
      <div class="diff-pane">
        <div class="pane-header">After</div>
        <div
          class="code-container"
          bind:this={afterPane}
          onscroll={handleAfterScroll}
          style="background-color: {themeBg}"
        >
          {#each diff.after.lines as line, i}
            {@const boundary = getLineBoundary(diff.ranges, 'after', i)}
            <div
              class="line"
              class:line-added={line.line_type === 'added'}
              class:range-start={boundary.isStart}
              class:range-end={boundary.isEnd}
            >
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
    min-width: 0;
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

  .pane-header {
    padding: 6px 12px;
    font-size: 11px;
    text-transform: uppercase;
    color: var(--text-muted);
    background-color: var(--diff-header-bg);
    border-bottom: 1px solid var(--border-primary);
  }

  .connector-gutter {
    width: 24px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    background-color: var(--bg-secondary);
  }

  .connector-header {
    height: 29px;
    background-color: var(--diff-header-bg);
    border-bottom: 1px solid var(--border-primary);
  }

  .connector-svg {
    flex: 1;
    width: 100%;
    overflow: visible;
  }

  .code-container {
    flex: 1;
    overflow: auto;
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    font-size: 13px;
    line-height: 1.5;
    min-width: 0;
    scrollbar-width: none;
    -ms-overflow-style: none;
  }

  .code-container::-webkit-scrollbar {
    display: none;
  }

  .line {
    display: flex;
    min-height: 20px;
    position: relative;
  }

  .line-content {
    flex: 1;
    padding: 0 12px;
    white-space: pre;
  }

  .content-added {
    background-color: var(--diff-added-overlay);
  }

  .content-removed {
    background-color: var(--diff-removed-overlay);
  }

  .line.range-start::before {
    content: '';
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 1px;
    background-color: var(--diff-line-number);
    opacity: 0.5;
  }

  .line.range-end::after {
    content: '';
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    height: 1px;
    background-color: var(--diff-line-number);
    opacity: 0.5;
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

  .empty-file-notice {
    padding: 20px;
    color: var(--text-muted);
    font-style: italic;
  }
</style>
