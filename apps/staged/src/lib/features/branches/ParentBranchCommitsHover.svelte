<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import type { Snippet } from 'svelte';
  import { listenToEvent } from '../../transport';
  import { listParentBranchCommits, type ParentBranchCommit } from '../../commands';
  import { formatRelativeTimeSeconds } from '../../shared/relativeTime.svelte';
  import type { BranchGitState } from '../../types';

  interface Props {
    branchId: string;
    baseBranch: string;
    count: number;
    children: Snippet;
  }

  let { branchId, baseBranch, count, children }: Props = $props();

  const MAX_ROWS = 25;
  const OPEN_DELAY_MS = 150;
  const CLOSE_DELAY_MS = 120;
  const VIEWPORT_PADDING = 8;
  const ANCHOR_GAP = 6;

  let open = $state(false);
  let commits = $state<ParentBranchCommit[] | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  let anchorEl = $state<HTMLSpanElement | null>(null);
  let popoverEl = $state<HTMLDivElement | null>(null);
  let left = $state(0);
  let top = $state(0);
  let positioned = $state(false);

  let openTimer: ReturnType<typeof setTimeout> | null = null;
  let closeTimer: ReturnType<typeof setTimeout> | null = null;
  let loadVersion = 0;

  function clamp(value: number, min: number, max: number): number {
    return Math.max(min, Math.min(value, max));
  }

  async function placePopover() {
    if (!open || !anchorEl) return;
    await tick();
    if (!popoverEl) return;
    const anchorRect = anchorEl.getBoundingClientRect();
    const popoverRect = popoverEl.getBoundingClientRect();
    left = clamp(
      anchorRect.left,
      VIEWPORT_PADDING,
      window.innerWidth - popoverRect.width - VIEWPORT_PADDING
    );
    const preferredTop = anchorRect.bottom + ANCHOR_GAP;
    const maxTop = window.innerHeight - popoverRect.height - VIEWPORT_PADDING;
    if (
      preferredTop > maxTop &&
      anchorRect.top - popoverRect.height - ANCHOR_GAP >= VIEWPORT_PADDING
    ) {
      top = anchorRect.top - popoverRect.height - ANCHOR_GAP;
    } else {
      top = clamp(preferredTop, VIEWPORT_PADDING, Math.max(VIEWPORT_PADDING, maxTop));
    }
    positioned = true;
  }

  async function loadCommits() {
    if (commits !== null || loading) return;
    loading = true;
    error = null;
    const version = ++loadVersion;
    try {
      const result = await listParentBranchCommits(branchId);
      if (version !== loadVersion) return;
      commits = result;
    } catch (e) {
      if (version !== loadVersion) return;
      error = e instanceof Error ? e.message : String(e);
    } finally {
      if (version === loadVersion) {
        loading = false;
      }
      await placePopover();
    }
  }

  function cancelTimers() {
    if (openTimer) {
      clearTimeout(openTimer);
      openTimer = null;
    }
    if (closeTimer) {
      clearTimeout(closeTimer);
      closeTimer = null;
    }
  }

  function scheduleOpen() {
    if (count <= 0) return;
    if (closeTimer) {
      clearTimeout(closeTimer);
      closeTimer = null;
    }
    if (open || openTimer) return;
    openTimer = setTimeout(() => {
      openTimer = null;
      open = true;
      positioned = false;
      void loadCommits();
      void placePopover();
    }, OPEN_DELAY_MS);
  }

  function scheduleClose() {
    if (openTimer) {
      clearTimeout(openTimer);
      openTimer = null;
    }
    if (!open || closeTimer) return;
    closeTimer = setTimeout(() => {
      closeTimer = null;
      open = false;
      positioned = false;
    }, CLOSE_DELAY_MS);
  }

  $effect(() => {
    const unlisten = listenToEvent<{ branchId: string; gitState: BranchGitState }>(
      'git-state-updated',
      (payload) => {
        if (payload.branchId !== branchId) return;
        commits = null;
        loadVersion++;
        if (open) {
          void loadCommits();
        }
      }
    );
    return () => unlisten();
  });

  $effect(() => {
    if (!open) return;
    const handler = () => {
      if (open) void placePopover();
    };
    window.addEventListener('resize', handler);
    window.addEventListener('scroll', handler, true);
    return () => {
      window.removeEventListener('resize', handler);
      window.removeEventListener('scroll', handler, true);
    };
  });

  onDestroy(() => {
    cancelTimers();
  });

  const visibleCommits = $derived(commits?.slice(0, MAX_ROWS) ?? []);
  const overflowCount = $derived(commits ? Math.max(0, count - visibleCommits.length) : 0);
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<span
  class="hover-anchor"
  bind:this={anchorEl}
  onmouseenter={scheduleOpen}
  onmouseleave={scheduleClose}
>
  {@render children()}
</span>

{#if open}
  <div
    class="commits-popover"
    role="tooltip"
    aria-label="Upstream commits on {baseBranch}"
    bind:this={popoverEl}
    style:left={`${left}px`}
    style:top={`${top}px`}
    style:visibility={positioned ? 'visible' : 'hidden'}
    onmouseenter={() => {
      if (closeTimer) {
        clearTimeout(closeTimer);
        closeTimer = null;
      }
    }}
    onmouseleave={scheduleClose}
  >
    <div class="popover-header">
      <span class="popover-title">Upstream changes in <strong>{baseBranch}</strong></span>
      <span class="popover-count">+{count}</span>
    </div>
    {#if loading && commits === null}
      <div class="popover-status">Loading…</div>
    {:else if error}
      <div class="popover-status popover-error">{error}</div>
    {:else if commits && commits.length === 0}
      <div class="popover-status">No upstream commits available.</div>
    {:else}
      <ul class="commit-list">
        {#each visibleCommits as commit (commit.sha)}
          <li class="commit-row">
            <div class="commit-main">
              <div class="commit-subject" title={commit.subject}>{commit.subject}</div>
              <div class="commit-meta">
                <span class="commit-author" title={commit.authorEmail}>{commit.author}</span>
                <span class="commit-dot">·</span>
                <span class="commit-time">{formatRelativeTimeSeconds(commit.timestamp)}</span>
              </div>
            </div>
            <span class="commit-sha">{commit.shortSha}</span>
          </li>
        {/each}
      </ul>
      {#if overflowCount > 0}
        <div class="popover-more">… +{overflowCount} more</div>
      {/if}
    {/if}
  </div>
{/if}

<style>
  .hover-anchor {
    display: inline-flex;
    align-items: center;
    min-width: 0;
    cursor: default;
  }

  .commits-popover {
    position: fixed;
    z-index: 1100;
    min-width: 280px;
    max-width: min(420px, calc(100vw - 16px));
    max-height: min(360px, calc(100vh - 16px));
    overflow-y: auto;
    padding: 6px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-primary);
    box-shadow: var(--shadow-elevated);
    color: var(--text-primary);
    font-size: var(--size-xs);
  }

  .popover-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 8px 6px;
    border-bottom: 1px solid var(--border-subtle);
    margin-bottom: 4px;
  }

  .popover-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-weight: 500;
  }

  .popover-count {
    font-weight: 600;
    color: var(--ui-accent);
    flex-shrink: 0;
  }

  .popover-status {
    padding: 8px;
    color: var(--text-muted);
  }

  .popover-error {
    color: var(--ui-warning, var(--status-modified));
  }

  .commit-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .commit-row {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 6px 8px;
    border-radius: 6px;
  }

  .commit-row + .commit-row {
    border-top: 1px solid var(--border-subtle);
  }

  .commit-main {
    flex: 1;
    min-width: 0;
  }

  .commit-subject {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-primary);
    font-weight: 500;
  }

  .commit-meta {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    color: var(--text-faint);
    font-size: var(--size-xxs, 11px);
    margin-top: 2px;
  }

  .commit-author {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .commit-dot {
    flex-shrink: 0;
  }

  .commit-time {
    flex-shrink: 0;
  }

  .commit-sha {
    flex-shrink: 0;
    font-family: var(--font-mono, ui-monospace, monospace);
    color: var(--text-faint);
    font-size: var(--size-xxs, 11px);
    padding-top: 2px;
  }

  .popover-more {
    padding: 6px 8px;
    color: var(--text-faint);
    border-top: 1px solid var(--border-subtle);
    margin-top: 2px;
  }
</style>
