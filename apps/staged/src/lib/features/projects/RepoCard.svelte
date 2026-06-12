<!--
  RepoCard.svelte — A card for a single repo on the home screen repos row.

  Shows repo short name (colored by badge hue), owner/repo subtitle, a
  pin/unpin toggle, and either a download button (when the repo has no
  local clone) or nothing.
-->
<script lang="ts">
  import Download from '@lucide/svelte/icons/download';
  import Pin from '@lucide/svelte/icons/pin';
  import PinOff from '@lucide/svelte/icons/pin-off';
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
  import { toast } from 'svelte-sonner';
  import { Button } from '$lib/components/ui/button';

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
      toast.error('Failed to update pin', {
        description: err instanceof Error ? err.message : String(err),
      });
    } finally {
      togglingPin = false;
    }
  }
</script>

<div
  class="repo-card"
  style="--accent: {accentColor}; --card-bg: {bgColor}; --card-bg-hover: {bgHoverColor}; --card-border: {borderColor}; --card-border-hover: {borderHoverColor};"
>
  <Button
    variant="ghost"
    size="icon-sm"
    class={[
      'absolute top-1.5 right-1.5 z-[2] size-[26px] hover:bg-[var(--bg-hover)] [&_svg]:!size-3.5',
      repo.pinned
        ? 'text-[var(--accent)] hover:text-[var(--accent)]'
        : 'text-[var(--text-faint)] hover:text-foreground',
    ]}
    title={repo.pinned ? 'Unpin repo' : 'Pin repo'}
    aria-label={repo.pinned ? 'Unpin repo' : 'Pin repo'}
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
  </Button>

  <span class="card-title" title={repo.shortName}>{repo.shortName}</span>

  <span class="card-subtitle" title={subtitle}>{subtitle}</span>

  {#if !repo.hasLocalClone}
    <div class="card-footer">
      <Button
        variant="outline"
        size="icon-sm"
        class="size-7 border-[var(--card-border)] bg-transparent text-[var(--accent)] shadow-none hover:bg-[var(--card-bg-hover)] hover:border-[var(--card-border-hover)] hover:text-[var(--accent)] [&_svg]:!size-3.5"
        title="Clone repo locally"
        aria-label="Clone repo locally"
        onclick={handleClone}
        disabled={cloning}
      >
        {#if cloning}
          <Spinner size={14} />
        {:else}
          <Download size={14} />
        {/if}
      </Button>
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
</style>
