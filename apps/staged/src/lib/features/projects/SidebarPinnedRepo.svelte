<!--
  SidebarPinnedRepo.svelte - A single pinned repo card in the sidebar.

  Displays the repo short name with badge hue accent, action buttons
  (new project, run, more menu), and supports drag-to-reorder.
  For repos without a local clone, shows a download button instead.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import Plus from '@lucide/svelte/icons/plus';
  import Play from '@lucide/svelte/icons/play';
  import MoreVertical from '@lucide/svelte/icons/more-vertical';
  import Download from '@lucide/svelte/icons/download';
  import Pin from '@lucide/svelte/icons/pin';
  import Copy from '@lucide/svelte/icons/copy';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import type { RepoHomeItem } from '../../types';
  import { darkMode } from '../../stores/isDark.svelte';
  import { badgeFg, badgeBorder } from '../../shared/badgeColors';
  import {
    getAvailableOpeners,
    openInApp,
    copyPathToClipboard,
    type OpenerApp,
  } from '../branches/branch';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import * as commands from '../../api/commands';
  import { toast } from 'svelte-sonner';
  import Spinner from '../../shared/Spinner.svelte';
  import { Button } from '$lib/components/ui/button';

  interface Props {
    repo: RepoHomeItem;
    onReorderStart?: (e: DragEvent) => void;
    onReorderOver?: (e: DragEvent) => void;
    onReorderDrop?: (e: DragEvent) => void;
    onReorderEnd?: (e: DragEvent) => void;
    onPinnedReposChanged?: () => void;
  }

  let {
    repo,
    onReorderStart,
    onReorderOver,
    onReorderDrop,
    onReorderEnd,
    onPinnedReposChanged,
  }: Props = $props();

  let openerApps = $state<OpenerApp[]>([]);
  let clonePath = $state<string | null>(null);
  let cloning = $state(false);
  let dragging = $state(false);
  let dragOver = $state(false);

  let stripColor = $derived(badgeFg(repo.hue, darkMode.value));
  let bgTint = $derived(
    darkMode.value ? `oklch(0.18 0.015 ${repo.hue})` : `oklch(0.98 0.008 ${repo.hue})`
  );
  let bgTintHover = $derived(
    darkMode.value ? `oklch(0.22 0.02 ${repo.hue})` : `oklch(0.96 0.012 ${repo.hue})`
  );
  let borderColor = $derived(badgeBorder(repo.hue, darkMode.value));

  let subpathLabel = $derived(repo.subpath ? repo.subpath : null);
  let subtitle = $derived(repo.subpath ? `${repo.githubRepo}/${repo.subpath}` : repo.githubRepo);

  onMount(() => {
    if (repo.hasLocalClone) {
      getAvailableOpeners().then((apps) => (openerApps = apps));
      commands
        .getRepoClonePath(repo.githubRepo)
        .then((path) => (clonePath = path))
        .catch((e) => console.error('[SidebarPinnedRepo] Failed to resolve clone path:', e));
    }
  });

  function openNewProjectForRepo() {
    window.dispatchEvent(
      new CustomEvent('staged:new-project', {
        detail: { githubRepo: repo.githubRepo, subpath: repo.subpath },
      })
    );
  }

  async function handleClone() {
    if (cloning) return;
    cloning = true;
    try {
      await commands.cloneRepoLocally(repo.githubRepo);
      onPinnedReposChanged?.();
    } catch (e) {
      console.error('[SidebarPinnedRepo] Failed to clone repo:', e);
      toast.error('Clone failed', {
        description: e instanceof Error ? e.message : String(e),
        duration: 5000,
      });
    } finally {
      cloning = false;
    }
  }

  async function handleUnpin() {
    try {
      await commands.unpinRepo(repo.githubRepo, repo.subpath);
      onPinnedReposChanged?.();
      window.dispatchEvent(new CustomEvent('staged:pinned-repos-changed'));
    } catch (e) {
      console.error('[SidebarPinnedRepo] Failed to unpin repo:', e);
    }
  }

  async function handleOpenInApp(path: string, app: OpenerApp) {
    try {
      await openInApp(path, app.id);
    } catch (e) {
      toast.error(`Failed to open in ${app.name}`, {
        description: e instanceof Error ? e.message : String(e),
        duration: 3000,
      });
    }
  }

  function handleDragStart(e: DragEvent) {
    dragging = true;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', `${repo.githubRepo}\t${repo.subpath}`);
    }
    onReorderStart?.(e);
  }

  function handleDragOver(e: DragEvent) {
    e.preventDefault();
    if (e.dataTransfer) {
      e.dataTransfer.dropEffect = 'move';
    }
    dragOver = true;
    onReorderOver?.(e);
  }

  function handleDragLeave() {
    dragOver = false;
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    onReorderDrop?.(e);
  }

  function handleDragEnd(e: DragEvent) {
    dragging = false;
    dragOver = false;
    onReorderEnd?.(e);
  }
</script>

<div
  class="pinned-repo-card"
  class:dragging
  class:drag-over={dragOver}
  draggable="true"
  role="listitem"
  style="--stripe-color: {stripColor}; --bg-tint: {bgTint}; --bg-tint-hover: {bgTintHover}; --border-color: {borderColor};"
  ondragstart={handleDragStart}
  ondragover={handleDragOver}
  ondragleave={handleDragLeave}
  ondrop={handleDrop}
  ondragend={handleDragEnd}
  data-repo={repo.githubRepo}
  data-subpath={repo.subpath}
>
  <div class="card-stripe"></div>

  <div class="card-content">
    <div class="card-name-row" title={subtitle}>
      <span class="repo-name">{repo.shortName}</span>
      {#if subpathLabel}
        <span class="subpath-badge">{subpathLabel}</span>
      {/if}
    </div>

    <div class="card-actions">
      <Button
        variant="ghost"
        class="size-[22px] rounded-[4px] p-0 text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)] [&_svg]:!size-3"
        title="New project"
        aria-label="New project"
        onclick={(e) => {
          e.stopPropagation();
          openNewProjectForRepo();
        }}
      >
        <Plus size={12} />
      </Button>

      {#if repo.hasLocalClone}
        <Button
          variant="ghost"
          class="size-[22px] rounded-[4px] p-0 text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--ui-success)] [&_svg]:!size-3"
          title="Run"
          aria-label="Run"
          onclick={(e) => {
            e.stopPropagation();
            // Run primary action — requires backend wiring (run action against main clone)
            toast.info('Run action', {
              description: 'Running actions against pinned repos is coming soon.',
              duration: 2000,
            });
          }}
        >
          <Play size={12} />
        </Button>
      {:else}
        <span class="inline-flex" title="Clone repo locally">
          <Button
            variant="ghost"
            class="size-[22px] rounded-[4px] p-0 text-[var(--text-secondary)] hover:bg-[var(--bg-hover)] hover:text-[var(--ui-accent)] [&_svg]:!size-3"
            aria-label="Clone repo locally"
            disabled={cloning}
            onclick={(e) => {
              e.stopPropagation();
              handleClone();
            }}
          >
            {#if cloning}
              <Spinner size={12} />
            {:else}
              <Download size={12} />
            {/if}
          </Button>
        </span>
      {/if}

      <DropdownMenu.Root>
        <DropdownMenu.Trigger
          class="inline-flex size-[22px] items-center justify-center rounded-[4px] bg-transparent text-[var(--text-secondary)] transition-colors hover:bg-[var(--bg-hover)] hover:text-[var(--text-primary)]"
          title="More options"
          aria-label="More options"
        >
          <MoreVertical size={12} />
        </DropdownMenu.Trigger>
        <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[172px]">
          {#if repo.hasLocalClone && clonePath}
            {@const path = clonePath}
            {#if openerApps.length > 0}
              <DropdownMenu.Sub>
                <DropdownMenu.SubTrigger>
                  <FolderOpen size={14} /> Open in…
                </DropdownMenu.SubTrigger>
                <DropdownMenu.SubContent class="min-w-[160px]">
                  {#each openerApps as app (app.id)}
                    <DropdownMenu.Item onSelect={() => handleOpenInApp(path, app)}>
                      {#if app.icon}
                        <img
                          src={app.icon}
                          alt=""
                          width="14"
                          height="14"
                          class="shrink-0 rounded-[3px]"
                        />
                      {/if}
                      {app.name}
                    </DropdownMenu.Item>
                  {/each}
                </DropdownMenu.SubContent>
              </DropdownMenu.Sub>
            {/if}
            <DropdownMenu.Item onSelect={() => copyPathToClipboard(path)}>
              <Copy size={14} /> Copy Path
            </DropdownMenu.Item>
            <DropdownMenu.Separator />
          {/if}
          <DropdownMenu.Item onSelect={handleUnpin}>
            <Pin size={14} /> Unpin Repo
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Root>
    </div>
  </div>
</div>

<style>
  .pinned-repo-card {
    position: relative;
    display: flex;
    align-items: stretch;
    border-radius: 6px;
    background: var(--bg-tint);
    border: 1px solid var(--border-color);
    cursor: grab;
    transition:
      background 0.15s ease,
      border-color 0.15s ease,
      box-shadow 0.15s ease;
    user-select: none;
    min-height: 32px;
  }

  .pinned-repo-card:hover {
    background: var(--bg-tint-hover);
  }

  .pinned-repo-card.dragging {
    opacity: 0.4;
    cursor: grabbing;
  }

  .pinned-repo-card.drag-over {
    box-shadow: 0 -2px 0 0 var(--ui-accent);
  }

  .card-stripe {
    width: 3px;
    flex-shrink: 0;
    border-radius: 6px 0 0 6px;
    background: var(--stripe-color);
  }

  .card-content {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex: 1;
    min-width: 0;
    padding: 4px 6px 4px 8px;
    gap: 6px;
  }

  .card-name-row {
    display: flex;
    align-items: center;
    gap: 4px;
    min-width: 0;
    flex: 1;
    color: inherit;
    text-align: left;
  }

  .repo-name {
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .subpath-badge {
    font-size: 9px;
    font-weight: 500;
    color: var(--text-tertiary);
    background: var(--bg-elevated);
    padding: 0px 4px;
    border-radius: 3px;
    white-space: nowrap;
    flex-shrink: 0;
  }

  .card-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    flex-shrink: 0;
  }
</style>
