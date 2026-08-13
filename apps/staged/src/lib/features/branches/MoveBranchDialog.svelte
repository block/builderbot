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
  import Cloud from '@lucide/svelte/icons/cloud';
  import Search from '@lucide/svelte/icons/search';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { projectDisplayName } from '../../shared/utils';
  import {
    branchRepoIdentity,
    filterMoveTargets,
    isMoveTargetChecking,
    moveTargetInvalidReason,
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
  let highlightedIndex = $state(-1);
  let error = $state<string | null>(null);
  let moving = $state(false);
  let searchElement = $state<HTMLInputElement | null>(null);
  let wasOpen = false;

  $effect(() => {
    if (open && !wasOpen) {
      query = '';
      selectedId = null;
      highlightedIndex = -1;
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
    if (filtered.length === 0) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlightedIndex = Math.min(highlightedIndex + 1, filtered.length - 1);
      selectProject(filtered[highlightedIndex]);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightedIndex = Math.max(highlightedIndex - 1, 0);
      selectProject(filtered[highlightedIndex]);
    }
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
  <Dialog.Content class="sm:max-w-[480px] gap-4">
    <Dialog.Header>
      <Dialog.Title>Move to Project</Dialog.Title>
      <Dialog.Description>
        {branch.branchName} moves with its notes, commits, reviews and sessions.
      </Dialog.Description>
    </Dialog.Header>

    <form class="move-form" onsubmit={handleSubmit}>
      <div class="search-field">
        <Search size={14} class="search-icon" />
        <Input
          id={searchId}
          bind:ref={searchElement}
          bind:value={query}
          disabled={moving}
          placeholder="Search projects…"
          autocomplete="off"
          class="pl-8"
          onkeydown={handleSearchKeydown}
        />
      </div>

      <div class="project-list" role="radiogroup" aria-label="Destination project">
        {#each filtered as project (project.id)}
          {@const repos = projectsDataStore.reposByProject.get(project.id) ?? []}
          <button
            type="button"
            class="project-row"
            class:selected={project.id === selectedId}
            role="radio"
            aria-checked={project.id === selectedId}
            disabled={moving}
            onclick={() => selectProject(project)}
          >
            <span class="project-name">
              {#if project.location === 'remote'}
                <Cloud size={14} />
              {/if}
              {projectDisplayName(project)}
            </span>
            {#if repos.length > 0}
              <span class="project-repos">
                {#each repos as repo (repo.id)}
                  <RepoLabel githubRepo={repo.githubRepo} subpath={repo.subpath} />
                {/each}
              </span>
            {/if}
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

      <Dialog.Footer>
        <Button type="button" variant="outline" onclick={requestClose} disabled={moving}>
          Cancel
        </Button>
        <Button type="submit" disabled={!selected || !!invalidReason || checking || moving}>
          {#if moving}
            <Spinner size={14} />
            <span>Moving...</span>
          {:else if checking}
            Checking...
          {:else}
            Move
          {/if}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  .move-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
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
  }

  .project-row {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
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

  .project-name {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-primary);
    font-size: var(--size-sm);
  }

  .project-repos {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    font-size: var(--size-xs);
  }

  .empty-state {
    margin: 0;
    padding: 12px 2px;
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
</style>
