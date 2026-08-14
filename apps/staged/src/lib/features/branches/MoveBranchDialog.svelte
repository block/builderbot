<!--
  MoveBranchDialog.svelte — pick the project a branch should move to.

  A move carries the branch's notes, commits, reviews, sessions and images with
  it, so the only thing to choose is the destination. Two destinations can't
  take it: a remote project (its branches share one Blox workspace) and a
  project that already has this branch's repo + subpath attached, which the
  backend's unique index would reject. Both are surfaced as a disabled Move
  button with the reason next to it rather than as a failed round-trip.
-->
<script lang="ts" module>
  let inputCounter = 0;
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import Spinner from '../../shared/Spinner.svelte';
  import ProjectRowContent from '../projects/ProjectRowContent.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import {
    branchRepoIdentity,
    filterMoveTargets,
    isMoveTargetChecking,
    moveTargetInvalidReason,
    nextMoveTargetIndex,
  } from './moveBranchTarget';
  import type { Branch, Project, ProjectRepo } from '../../types';

  interface Props {
    open: boolean;
    branch: Branch;
    repoLabel?: ProjectRepo | null;
    onMove?: (targetProjectId: string) => void | Promise<void>;
  }

  let { open = $bindable(false), branch, repoLabel = null, onMove }: Props = $props();

  const searchId = `move-branch-search-${++inputCounter}`;
  let query = $state('');
  let selectedId = $state<string | null>(null);
  let error = $state<string | null>(null);
  let moving = $state(false);
  let searchElement = $state<HTMLInputElement | null>(null);
  let wasOpen = false;

  $effect(() => {
    if (open && !wasOpen) {
      query = '';
      selectedId = null;
      error = null;
      moving = false;
      // Repos hydrate lazily, so the duplicate-repo check can't be trusted
      // until every candidate's repos have actually been fetched.
      void projectsDataStore.ensureProjectsHydrated();
      void tick().then(() => searchElement?.focus());
    }
    wasOpen = open;
  });

  let sourceProject = $derived(
    projectsDataStore.projects.find((p) => p.id === branch.projectId) ?? null
  );
  let branchRepo = $derived(branchRepoIdentity(repoLabel, sourceProject));

  let candidates = $derived(projectsDataStore.projects.filter((p) => p.id !== branch.projectId));
  let filtered = $derived(filterMoveTargets(candidates, projectsDataStore.reposByProject, query));
  let selected = $derived(filtered.find((p) => p.id === selectedId) ?? null);
  // Derived, not tracked: a stale cursor could outlive the row it named — the
  // query narrowing the list, or a click moving the selection out from under it.
  let highlightedIndex = $derived(filtered.findIndex((p) => p.id === selectedId));
  let selectedRepos = $derived(
    selected ? projectsDataStore.reposByProject.get(selected.id) : undefined
  );

  let invalidReason = $derived(
    selected ? moveTargetInvalidReason(selected, branchRepo, selectedRepos) : null
  );
  let checking = $derived(!!selected && isMoveTargetChecking(selected, selectedRepos));

  function errorMessage(e: unknown): string {
    if (e instanceof Error) return e.message;
    return String(e);
  }

  function requestClose() {
    if (moving) return;
    open = false;
  }

  function selectProject(project: Project) {
    selectedId = project.id;
    error = null;
  }

  function handleSearchKeydown(e: KeyboardEvent) {
    const delta = e.key === 'ArrowDown' ? 1 : e.key === 'ArrowUp' ? -1 : 0;
    if (delta === 0) return;
    e.preventDefault();
    const next = nextMoveTargetIndex(highlightedIndex, delta, filtered.length);
    if (next < 0) return;
    selectProject(filtered[next]);
  }

  async function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    if (moving || !selected || invalidReason || checking) return;

    moving = true;
    error = null;
    try {
      await onMove?.(selected.id);
      open = false;
    } catch (e) {
      error = errorMessage(e);
    } finally {
      moving = false;
    }
  }
</script>

<Dialog.Root
  {open}
  onOpenChange={(nextOpen) => {
    if (nextOpen) {
      open = true;
    } else {
      requestClose();
    }
  }}
>
  <Dialog.Content
    class="sm:max-w-[480px] p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
  >
    <header class="modal-header">
      <Dialog.Title class="text-[var(--size-sm)] font-semibold text-foreground">
        Move to Project
      </Dialog.Title>
      <Button
        variant="ghost"
        size="icon"
        class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[768px]:size-10 [&_svg]:!size-[18px]"
        title="Close"
        aria-label="Close"
        onclick={requestClose}
        disabled={moving}
      >
        <X size={18} />
      </Button>
    </header>

    <form class="modal-body" onsubmit={handleSubmit}>
      <Dialog.Description class="text-[var(--size-sm)]">
        {branch.branchName} moves with its notes, commits, reviews and sessions.
      </Dialog.Description>

      <div class="search-field">
        <Search size={14} class="search-icon" />
        <Input
          id={searchId}
          bind:ref={searchElement}
          bind:value={query}
          disabled={moving}
          placeholder="Search projects…"
          autocomplete="off"
          class="pl-8 bg-[var(--bg-primary)] focus-visible:ring-0 focus-visible:border-[var(--border-emphasis)]"
          onkeydown={handleSearchKeydown}
        />
      </div>

      <div class="project-list" role="radiogroup" aria-label="Destination project">
        {#each filtered as project (project.id)}
          <button
            type="button"
            class="project-row"
            class:selected={project.id === selectedId}
            role="radio"
            aria-checked={project.id === selectedId}
            disabled={moving}
            onclick={() => selectProject(project)}
          >
            <ProjectRowContent {project} />
          </button>
        {:else}
          <p class="empty-state">
            {#if query.trim()}
              No projects matching "{query.trim()}"
            {:else}
              No other projects yet.
            {/if}
          </p>
        {/each}
      </div>

      {#if error}
        <p class="move-error" role="alert">{error}</p>
      {:else if invalidReason}
        <p class="move-warning" role="alert">{invalidReason}</p>
      {/if}

      <div class="form-actions">
        <Button
          type="button"
          variant="outline"
          class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
          onclick={requestClose}
          disabled={moving}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          variant="accent"
          class="gap-1.5 px-4 py-2 text-sm max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
          disabled={!selected || !!invalidReason || checking || moving}
        >
          {#if moving}
            <Spinner size={14} />
            <span>Moving...</span>
          {:else if checking}
            Checking...
          {:else}
            Move
          {/if}
        </Button>
      </div>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px 18px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-body {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
    min-height: 0;
  }

  .search-field {
    position: relative;
    display: flex;
    align-items: center;
  }

  .search-field :global(.search-icon) {
    position: absolute;
    left: 10px;
    color: var(--text-faint);
    pointer-events: none;
  }

  .project-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 260px;
    overflow-y: auto;
    padding: 4px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
  }

  .project-row {
    display: flex;
    align-items: center;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    text-align: left;
    cursor: pointer;
  }

  .project-row:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .project-row.selected {
    border-color: var(--ui-accent);
    background: var(--bg-hover);
  }

  .project-row:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }

  .empty-state {
    margin: 0;
    padding: 12px 10px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .move-error {
    margin: 0;
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  .move-warning {
    margin: 0;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
    flex-shrink: 0;
  }

  @media (max-width: 768px) {
    .modal-header {
      padding: 12px 16px;
    }

    .modal-body {
      padding: 16px;
    }

    /* Full-screen on mobile: let the list take the leftover height instead of
       capping at the desktop popover size. */
    .project-list {
      max-height: none;
      flex: 1;
      min-height: 0;
    }
  }
</style>
