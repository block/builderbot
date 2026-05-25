<!--
  RepoCard.svelte — A card for a single repo on the home screen repos row.

  Shows repo short name (colored by badge hue), owner/repo subtitle, a
  pin/unpin toggle, and either a download button (when the repo has no
  local clone) or nothing.
-->
<script lang="ts">
  import { Download, Pin, PinOff } from 'lucide-svelte';
  import type { RepoHomeItem } from '../../types';
  import { darkMode } from '../../stores/isDark.svelte';
  import {
    badgeFg,
    badgeBg,
    badgeBgHover,
    badgeBorder,
    badgeBorderHover,
  } from '../../shared/badgeColors';
  import Spinner from '../../shared/Spinner.svelte';
  import * as commands from '../../api/commands';
  import { alerts } from '../../shared/alerts.svelte';

  interface Props {
    repo: RepoHomeItem;
    onclone: () => void | Promise<void>;
    onPinChange?: () => void;
  }

  let { repo, onclone, onPinChange }: Props = $props();

  let cloning = $state(false);
  let togglingPin = $state(false);

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
      await onclone();
    } finally {
      cloning = false;
    }
  }

  async function handleTogglePin(e: MouseEvent) {
    e.stopPropagation();
    if (togglingPin) return;
    togglingPin = true;
    try {
      if (repo.pinned) {
        await commands.unpinRepo(repo.githubRepo, repo.subpath);
      } else {
        await commands.pinRepo(repo.githubRepo, repo.subpath);
      }
      onPinChange?.();
      window.dispatchEvent(new CustomEvent('staged:pinned-repos-changed'));
    } catch (err) {
      console.error('[RepoCard] Failed to toggle pin:', err);
      alerts.show({
        tone: 'error',
        title: 'Failed to update pin',
        message: err instanceof Error ? err.message : String(err),
      });
    } finally {
      togglingPin = false;
    }
  }
</script>

<div
  class="repo-card"
  style="--accent: {accentColor}; --card-bg: {bgColor}; --card-bg-hover: {bgHoverColor}; --card-border: {borderColor}; --card-border-hover: {borderHoverColor};"
  title={subtitle}
>
  <button
    class="pin-toggle"
    class:pinned={repo.pinned}
    title={repo.pinned ? 'Unpin repo' : 'Pin repo'}
    onclick={handleTogglePin}
    disabled={togglingPin}
  >
    {#if togglingPin}
      <Spinner size={14} />
    {:else if repo.pinned}
      <Pin size={14} />
    {:else}
      <PinOff size={14} />
    {/if}
  </button>

  <span class="card-title">{repo.shortName}</span>

  <span class="card-subtitle">{subtitle}</span>

  {#if !repo.hasLocalClone}
    <div class="card-footer">
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
    </div>
  {/if}
</div>

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
    transition: all 0.15s ease;
    box-sizing: border-box;
  }

  .pin-toggle {
    position: absolute;
    top: 6px;
    right: 6px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
    transition: all 0.15s ease;
    z-index: 2;
  }

  .pin-toggle:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .pin-toggle.pinned {
    color: var(--accent);
  }

  .pin-toggle.pinned:hover:not(:disabled) {
    color: var(--accent);
    background: var(--bg-hover);
  }

  .pin-toggle:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .card-title {
    font-size: var(--size-md);
    font-weight: 700;
    color: var(--accent);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding-right: 28px;
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
