<!--
  RepoCard.svelte — A card for a single repo on the home screen repos row.

  Shows repo short name (colored by badge hue), owner/repo subtitle,
  latest commit info or a download button for unclonable repos,
  and a dirty-state indicator.
-->
<script lang="ts">
  import { Download, AlertTriangle, Plus } from 'lucide-svelte';
  import type { RepoHomeItem } from '../../types';
  import { formatRelativeTimeSeconds } from '../../shared/relativeTime.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import {
    badgeFg,
    badgeBg,
    badgeBgHover,
    badgeBorder,
    badgeBorderHover,
  } from '../../shared/badgeColors';
  import Spinner from '../../shared/Spinner.svelte';

  interface Props {
    repo: RepoHomeItem;
    onclick: () => void;
    onclone: () => void;
  }

  let { repo, onclick, onclone }: Props = $props();

  let cloning = $state(false);
  let showDirtyPopover = $state(false);

  function handleDirtyClick(e: MouseEvent) {
    e.stopPropagation();
    showDirtyPopover = !showDirtyPopover;
  }

  function closeDirtyPopover() {
    showDirtyPopover = false;
  }

  function handleAddProjectFromChanges(e: MouseEvent) {
    e.stopPropagation();
    showDirtyPopover = false;
    window.dispatchEvent(
      new CustomEvent('staged:new-project', {
        detail: { githubRepo: repo.githubRepo, subpath: repo.subpath },
      })
    );
  }

  let subtitle = $derived.by(() => {
    const base = repo.githubRepo;
    if (repo.subpath) return `${base}/${repo.subpath}`;
    return base;
  });

  let accentColor = $derived(badgeFg(repo.hue, darkMode.value));
  let bgColor = $derived(badgeBg(repo.hue, darkMode.value));
  let bgHoverColor = $derived(badgeBgHover(repo.hue, darkMode.value));
  let borderColor = $derived(badgeBorder(repo.hue, darkMode.value));
  let borderHoverColor = $derived(badgeBorderHover(repo.hue, darkMode.value));

  async function handleClone(e: MouseEvent) {
    e.stopPropagation();
    if (cloning) return;
    cloning = true;
    try {
      onclone();
    } finally {
      // The parent will refresh the repo list after cloning completes,
      // but reset local state in case the card stays mounted.
      cloning = false;
    }
  }
</script>

<button
  class="repo-card"
  style="--accent: {accentColor}; --card-bg: {bgColor}; --card-bg-hover: {bgHoverColor}; --card-border: {borderColor}; --card-border-hover: {borderHoverColor};"
  {onclick}
  title={subtitle}
>
  {#if repo.isDirty}
    <button
      class="dirty-indicator"
      title="Uncommitted changes on main branch"
      onclick={handleDirtyClick}
    >
      <AlertTriangle size={12} />
    </button>
    {#if showDirtyPopover}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="dirty-popover-backdrop" onclick={closeDirtyPopover} onkeydown={() => {}}></div>
      <div class="dirty-popover" onclick={(e) => e.stopPropagation()}>
        <p class="dirty-popover-message">
          Staged maintains the main branch. Uncommitted changes may be lost.
        </p>
        <button class="dirty-popover-action" onclick={handleAddProjectFromChanges}>
          <Plus size={12} />
          Add project from changes
        </button>
      </div>
    {/if}
  {/if}

  {#if repo.pinned}
    <span class="pin-indicator" title="Pinned"></span>
  {/if}

  <span class="card-title">{repo.shortName}</span>

  <span class="card-subtitle">{subtitle}</span>

  <div class="card-footer">
    {#if !repo.hasLocalClone}
      <button
        class="download-btn"
        title="Clone repo locally"
        onclick={handleClone}
        disabled={cloning}
      >
        {#if cloning}
          <Spinner size={14} />
        {:else}
          <Download size={14} />
        {/if}
      </button>
    {:else if repo.latestCommit}
      <span class="commit-info">
        <span class="commit-subject" title={repo.latestCommit.subject}>
          {repo.latestCommit.subject}
        </span>
        <span class="commit-time">
          {formatRelativeTimeSeconds(repo.latestCommit.timestamp)}
        </span>
      </span>
    {/if}
  </div>
</button>

<style>
  .repo-card {
    position: relative;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    width: 200px;
    min-height: 100px;
    padding: 14px;
    border: 1px solid var(--card-border);
    border-radius: 10px;
    background: var(--card-bg);
    color: inherit;
    text-align: left;
    cursor: pointer;
    transition: all 0.15s ease;
    box-sizing: border-box;
  }

  .repo-card:hover {
    background: var(--card-bg-hover);
    border-color: var(--card-border-hover);
  }

  .pin-indicator {
    position: absolute;
    top: 8px;
    right: 8px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    opacity: 0.6;
  }

  .dirty-indicator {
    position: absolute;
    top: 7px;
    right: 7px;
    color: var(--ui-warning, #e5a100);
    display: flex;
    align-items: center;
    background: none;
    border: none;
    padding: 2px;
    border-radius: 4px;
    cursor: pointer;
    z-index: 2;
    transition: background 0.12s ease;
  }

  .dirty-indicator:hover {
    background: var(--bg-hover);
  }

  .dirty-popover-backdrop {
    position: fixed;
    inset: 0;
    z-index: 99;
  }

  .dirty-popover {
    position: absolute;
    top: 24px;
    right: 4px;
    z-index: 100;
    width: 220px;
    padding: 10px 12px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .dirty-popover-message {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-secondary);
    line-height: 1.4;
  }

  .dirty-popover-action {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.12s ease;
    white-space: nowrap;
  }

  .dirty-popover-action:hover {
    background: var(--bg-hover);
    border-color: var(--border-muted);
  }

  .card-title {
    font-size: var(--size-md);
    font-weight: 700;
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding-right: 16px;
  }

  .card-subtitle {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .card-footer {
    margin-top: auto;
    display: flex;
    align-items: center;
    min-height: 20px;
  }

  .commit-info {
    display: flex;
    flex-direction: column;
    gap: 1px;
    overflow: hidden;
    width: 100%;
  }

  .commit-subject {
    font-size: var(--size-xs);
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .commit-time {
    font-size: 10px;
    color: var(--text-faint);
  }

  .download-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 1px solid var(--card-border);
    border-radius: 6px;
    background: transparent;
    color: var(--accent);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .download-btn:hover:not(:disabled) {
    background: var(--card-bg-hover);
    border-color: var(--card-border-hover);
  }

  .download-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
