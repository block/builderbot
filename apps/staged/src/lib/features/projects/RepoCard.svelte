<!--
  RepoCard.svelte — the card for a single repo, shared by every repos surface:
  the home screen repos row, the All Repos grid, and the pinned repos list in
  the projects sidebar.

  The card is the full repo path (rendered by the shared RepoLabel, wrapped over
  as many lines as it needs) above a row of actions: new project, run or clone,
  pin toggle, and a more menu carrying every repo action plus the local-clone
  openers. Card tint, border and accent all come from the repo's badge hue.

  Pass `reorderable` to make the card a drag-to-reorder handle (the sidebar's
  pinned list); the drag callbacks are only wired up in that mode. Pass
  `hidePinButton` to drop the pin toggle from the action row (the sidebar keeps
  unpin in the more menu only).
-->
<script lang="ts">
  import Plus from '@lucide/svelte/icons/plus';
  import Play from '@lucide/svelte/icons/play';
  import MoreVertical from '@lucide/svelte/icons/more-vertical';
  import Download from '@lucide/svelte/icons/download';
  import Pin from '@lucide/svelte/icons/pin';
  import PinOff from '@lucide/svelte/icons/pin-off';
  import Copy from '@lucide/svelte/icons/copy';
  import FolderOpen from '@lucide/svelte/icons/folder-open';
  import type { RepoHomeItem } from '../../types';
  import { darkMode } from '../../stores/isDark.svelte';
  import {
    badgeFg,
    badgeBg,
    badgeBgHover,
    badgeBorder,
    badgeBorderHover,
    badgeShortcutBg,
  } from '../../shared/badgeColors';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import {
    getAvailableOpeners,
    openInApp,
    copyPathToClipboard,
    type OpenerApp,
  } from '../branches/branch';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import type { NewProjectEventDetail } from './newProjectEvent';
  import * as commands from '../../api/commands';
  import { toast } from 'svelte-sonner';
  import { Button } from '$lib/components/ui/button';

  interface Props {
    repo: RepoHomeItem;
    /** Called after this card pins, unpins or clones the repo. */
    onChange?: () => void;
    /** Drop the pin toggle from the action row; pin/unpin stays in the more menu. */
    hidePinButton?: boolean;
    /** Turn the card into a drag-to-reorder handle. */
    reorderable?: boolean;
    onReorderStart?: (e: DragEvent) => void;
    onReorderOver?: (e: DragEvent) => void;
    onReorderDrop?: (e: DragEvent) => void;
    onReorderEnd?: (e: DragEvent) => void;
  }

  let {
    repo,
    onChange,
    hidePinButton = false,
    reorderable = false,
    onReorderStart,
    onReorderOver,
    onReorderDrop,
    onReorderEnd,
  }: Props = $props();

  let openerApps = $state<OpenerApp[]>([]);
  let clonePath = $state<string | null>(null);
  let cloneDetailsLoaded = $state(false);
  let cloning = $state(false);
  let togglingPin = $state(false);
  let dragging = $state(false);
  let dragOver = $state(false);

  let accentColor = $derived(badgeFg(repo.hue, darkMode.value));
  let bgColor = $derived(badgeBg(repo.hue, darkMode.value));
  let bgHoverColor = $derived(badgeBgHover(repo.hue, darkMode.value));
  /** One tint step past the card's own hover so in-card buttons stay legible. */
  let bgStrongColor = $derived(badgeShortcutBg(repo.hue, darkMode.value));
  let borderColor = $derived(badgeBorder(repo.hue, darkMode.value));
  let borderHoverColor = $derived(badgeBorderHover(repo.hue, darkMode.value));

  /** Resolve the clone path and opener apps the first time the menu opens. */
  async function loadCloneDetails() {
    if (!repo.hasLocalClone || cloneDetailsLoaded) return;
    cloneDetailsLoaded = true;
    try {
      const [apps, path] = await Promise.all([
        getAvailableOpeners(),
        commands.getRepoClonePath(repo.githubRepo),
      ]);
      openerApps = apps;
      clonePath = path;
    } catch (e) {
      console.error('[RepoCard] Failed to resolve clone details:', e);
      cloneDetailsLoaded = false;
    }
  }

  function openNewProjectForRepo() {
    const detail: NewProjectEventDetail = { githubRepo: repo.githubRepo, subpath: repo.subpath };
    window.dispatchEvent(new CustomEvent('staged:new-project', { detail }));
  }

  function handleRun() {
    // Run primary action — requires backend wiring (run action against main clone)
    toast.info('Run action', {
      description: 'Running actions against repos is coming soon.',
      duration: 2000,
    });
  }

  async function handleClone() {
    if (cloning) return;
    cloning = true;
    try {
      await commands.cloneRepoLocally(repo.githubRepo);
      onChange?.();
    } catch (e) {
      console.error('[RepoCard] Failed to clone repo:', e);
      toast.error('Clone failed', {
        description: e instanceof Error ? e.message : String(e),
        duration: 5000,
      });
    } finally {
      cloning = false;
    }
  }

  async function handleTogglePin() {
    if (togglingPin) return;
    togglingPin = true;
    try {
      if (repo.pinned) {
        await commands.unpinRepo(repo.githubRepo, repo.subpath);
      } else {
        await commands.pinRepo(repo.githubRepo, repo.subpath);
      }
      onChange?.();
      window.dispatchEvent(new CustomEvent('staged:pinned-repos-changed'));
    } catch (e) {
      console.error('[RepoCard] Failed to toggle pin:', e);
      toast.error('Failed to update pin', {
        description: e instanceof Error ? e.message : String(e),
      });
    } finally {
      togglingPin = false;
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
  class="repo-card"
  class:reorderable
  class:dragging
  class:drag-over={dragOver}
  draggable={reorderable}
  role={reorderable ? 'listitem' : undefined}
  style="--accent: {accentColor}; --card-bg: {bgColor}; --card-bg-hover: {bgHoverColor}; --card-bg-strong: {bgStrongColor}; --card-border: {borderColor}; --card-border-hover: {borderHoverColor};"
  ondragstart={reorderable ? handleDragStart : undefined}
  ondragover={reorderable ? handleDragOver : undefined}
  ondragleave={reorderable ? handleDragLeave : undefined}
  ondrop={reorderable ? handleDrop : undefined}
  ondragend={reorderable ? handleDragEnd : undefined}
  data-repo={repo.githubRepo}
  data-subpath={repo.subpath}
>
  <span class="card-title">
    <RepoLabel githubRepo={repo.githubRepo} subpath={repo.subpath || null} wrap />
  </span>

  <div class="card-actions">
    <Button
      variant="ghost"
      class="size-[22px] rounded-[4px] p-0 text-[var(--text-secondary)] hover:bg-[var(--card-bg-strong)] hover:text-[var(--text-primary)] [&_svg]:!size-3"
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
        class="size-[22px] rounded-[4px] p-0 text-[var(--text-secondary)] hover:bg-[var(--card-bg-strong)] hover:text-[var(--ui-success)] [&_svg]:!size-3"
        title="Run"
        aria-label="Run"
        onclick={(e) => {
          e.stopPropagation();
          handleRun();
        }}
      >
        <Play size={12} />
      </Button>
    {:else}
      <span class="inline-flex" title="Clone repo locally">
        <Button
          variant="ghost"
          class="size-[22px] rounded-[4px] p-0 text-[var(--text-secondary)] hover:bg-[var(--card-bg-strong)] hover:text-[var(--ui-accent)] [&_svg]:!size-3"
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

    {#if !hidePinButton}
      <Button
        variant="ghost"
        class={[
          'size-[22px] rounded-[4px] p-0 hover:bg-[var(--card-bg-strong)] [&_svg]:!size-3',
          repo.pinned
            ? 'text-[var(--accent)] hover:text-[var(--accent)]'
            : 'text-[var(--text-faint)] hover:text-[var(--text-primary)]',
        ]}
        title={repo.pinned ? 'Unpin repo' : 'Pin repo'}
        aria-label={repo.pinned ? 'Unpin repo' : 'Pin repo'}
        disabled={togglingPin}
        onclick={(e) => {
          e.stopPropagation();
          handleTogglePin();
        }}
      >
        {#if togglingPin}
          <Spinner size={12} />
        {:else if repo.pinned}
          <Pin size={12} />
        {:else}
          <PinOff size={12} />
        {/if}
      </Button>
    {/if}

    <DropdownMenu.Root
      onOpenChange={(open) => {
        if (open) void loadCloneDetails();
      }}
    >
      <DropdownMenu.Trigger
        class="inline-flex size-[22px] items-center justify-center rounded-[4px] bg-transparent text-[var(--text-secondary)] transition-colors hover:bg-[var(--card-bg-strong)] hover:text-[var(--text-primary)]"
        title="More options"
        aria-label="More options"
      >
        <MoreVertical size={12} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Content align="end" sideOffset={4} class="min-w-[172px]">
        <DropdownMenu.Item onSelect={openNewProjectForRepo}>
          <Plus size={14} /> New Project
        </DropdownMenu.Item>
        {#if repo.hasLocalClone}
          <DropdownMenu.Item onSelect={handleRun}>
            <Play size={14} /> Run
          </DropdownMenu.Item>
        {:else}
          <DropdownMenu.Item disabled={cloning} onSelect={handleClone}>
            <Download size={14} /> Clone Repo
          </DropdownMenu.Item>
        {/if}
        <DropdownMenu.Item disabled={togglingPin} onSelect={handleTogglePin}>
          {#if repo.pinned}
            <PinOff size={14} /> Unpin Repo
          {:else}
            <Pin size={14} /> Pin Repo
          {/if}
        </DropdownMenu.Item>
        {#if repo.hasLocalClone}
          <DropdownMenu.Separator />
          {#if clonePath}
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
          {:else}
            <DropdownMenu.Item disabled>Loading…</DropdownMenu.Item>
          {/if}
        {/if}
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  </div>
</div>

<style>
  .repo-card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 10px 8px;
    border: 1px solid var(--card-border);
    border-radius: 8px;
    background: var(--card-bg);
    color: inherit;
    text-align: left;
    box-sizing: border-box;
    transition:
      background 0.15s ease,
      border-color 0.15s ease,
      box-shadow 0.15s ease;
  }

  .repo-card:hover {
    background: var(--card-bg-hover);
    border-color: var(--card-border-hover);
  }

  .repo-card.reorderable {
    cursor: grab;
    user-select: none;
  }

  .repo-card.dragging {
    opacity: 0.4;
    cursor: grabbing;
  }

  .repo-card.drag-over {
    box-shadow: 0 -2px 0 0 var(--ui-accent);
  }

  .card-title {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: var(--size-sm);
    font-weight: 600;
    line-height: 1.35;
  }

  /* Emphasize the distinguishing path segment in the repo's badge hue. */
  .card-title :global(.repo-label-emphasis) {
    color: var(--accent);
    font-weight: 700;
  }

  .card-actions {
    margin-top: auto;
    display: flex;
    align-items: center;
    gap: 2px;
  }
</style>
