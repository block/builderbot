<!--
  SidebarPinnedRepo.svelte - A single pinned repo card in the sidebar.

  Displays the repo short name with badge hue accent, action buttons
  (new project, run, more menu), and supports drag-to-reorder.
  For repos without a local clone, shows a download button instead.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { Plus, Play, MoreVertical, Download, Pin, Copy, FolderOpen } from 'lucide-svelte';
  import type { RepoHomeItem } from '../../types';
  import { darkMode } from '../../stores/isDark.svelte';
  import { badgeFg, badgeBorder } from '../../shared/badgeColors';
  import ContextMenu from '../../shared/menu/ContextMenu.svelte';
  import type { MenuItem } from '../../shared/menu/types';
  import {
    getAvailableOpeners,
    openInApp,
    copyPathToClipboard,
    type OpenerApp,
  } from '../branches/branch';
  import * as commands from '../../api/commands';
  import { alerts } from '../../shared/alerts.svelte';
  import Spinner from '../../shared/Spinner.svelte';

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

  let contextMenu = $state<ReturnType<typeof ContextMenu> | undefined>();
  let openerApps = $state<OpenerApp[]>([]);
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
      alerts.show({
        tone: 'error',
        title: 'Clone failed',
        message: e instanceof Error ? e.message : String(e),
        durationMs: 5000,
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

  function openContextMenu(event: MouseEvent) {
    event.preventDefault();
    event.stopPropagation();

    const items: MenuItem[] = [];

    if (repo.hasLocalClone) {
      // Open-in submenu
      if (openerApps.length > 0) {
        items.push({
          type: 'submenu',
          label: 'Open in\u2026',
          icon: FolderOpen,
          children: openerApps.map((app) => ({
            type: 'action' as const,
            label: app.name,
            iconSrc: app.icon ?? undefined,
            onSelect: async () => {
              // Derive path from githubRepo — best effort until backend provides it
              try {
                // Use the repo's githubRepo slug to compute expected path
                await openInApp(`~/.staged/clones/${repo.githubRepo}`, app.id);
              } catch (e) {
                alerts.show({
                  tone: 'error',
                  title: `Failed to open in ${app.name}`,
                  message: e instanceof Error ? e.message : String(e),
                  durationMs: 3000,
                });
              }
            },
          })),
        });
      }

      // Copy path
      items.push({
        type: 'action',
        label: 'Copy Path',
        icon: Copy,
        onSelect: () => copyPathToClipboard(`~/.staged/clones/${repo.githubRepo}`),
      });

      items.push({ type: 'separator' });
    }

    // Unpin
    items.push({
      type: 'action',
      label: 'Unpin Repo',
      icon: Pin,
      onSelect: handleUnpin,
    });

    contextMenu?.open({
      x: event.clientX,
      y: event.clientY,
      items,
      ariaLabel: `Actions for ${repo.shortName}`,
    });
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
      <button
        class="action-btn"
        title="New project"
        onclick={(e) => {
          e.stopPropagation();
          openNewProjectForRepo();
        }}
      >
        <Plus size={12} />
      </button>

      {#if repo.hasLocalClone}
        <button
          class="action-btn run-btn"
          title="Run"
          onclick={(e) => {
            e.stopPropagation();
            // Run primary action — requires backend wiring (run action against main clone)
            alerts.show({
              tone: 'info',
              title: 'Run action',
              message: 'Running actions against pinned repos is coming soon.',
              durationMs: 2000,
            });
          }}
        >
          <Play size={12} />
        </button>
      {:else}
        <button
          class="action-btn download-btn"
          title="Clone repo locally"
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
        </button>
      {/if}

      <button class="action-btn more-btn" title="More options" onclick={openContextMenu}>
        <MoreVertical size={12} />
      </button>
    </div>
  </div>
</div>

<ContextMenu bind:this={contextMenu} ariaLabel="Pinned repo actions" minWidth={172} />

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

  .action-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 0;
    transition:
      background 0.12s ease,
      color 0.12s ease;
  }

  .action-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .action-btn:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: -1px;
  }

  .action-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .run-btn:hover {
    color: var(--ui-success, oklch(0.65 0.15 145));
  }

  .download-btn:hover {
    color: var(--ui-accent);
  }
</style>
