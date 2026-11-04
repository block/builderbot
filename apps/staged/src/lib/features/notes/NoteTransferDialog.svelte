<!--
  NoteTransferDialog.svelte — pick the project a note moves or copies to.

  The same picker serves both actions; `mode` only changes the words. The
  search box doubles as the way to create a project: when what's typed names
  no existing project, a "Create project" row is offered at the end of the
  list, and confirming creates it (through the regular create-project path)
  before the note goes there. Whenever confirming would create a project —
  that row picked, or the only row left for Enter — a Local / Remote choice
  is offered for it when remote projects are available (the `sq` CLI is
  installed). Like the new-project form, it starts on the location the last
  new project was created with (`preferences.newProjectLocation`, which
  creating one here with Remote on offer updates), showing local while Remote
  can't be picked. A remote project starts from the note's repo, looked up
  from the stored note on each open through `loadNoteRepo` (the host's idea
  of the repo can be missing or stale), so Remote waits on that lookup.
  `onPrepare` runs before anything is created, so a transfer refused up front
  leaves no project behind — a remote one starts provisioning its workspace
  as soon as it exists. Failures — from those checks, creating the project,
  adding the note's repo, or the transfer itself — stay inline so the choice
  can be retried, and one that comes after the project was created says so;
  the backend guarantees a failed transfer left the note where it was.
-->
<script lang="ts" module>
  let inputCounter = 0;
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import FolderPlus from '@lucide/svelte/icons/folder-plus';
  import Search from '@lucide/svelte/icons/search';
  import X from '@lucide/svelte/icons/x';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import Spinner from '../../shared/Spinner.svelte';
  import { focusAfterDialogEnter } from '../../shared/focusAfterDialogEnter';
  import ProjectLocationPicker from '../projects/ProjectLocationPicker.svelte';
  import ProjectRowContent from '../projects/ProjectRowContent.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { sqState } from '../settings/sq.svelte';
  import { preferences, setNewProjectLocation } from '../settings/preferences.svelte';
  import * as commands from '../../api/commands';
  import { filterMoveTargets, nextMoveTargetIndex } from '../branches/moveBranchTarget';
  import {
    createProjectOption,
    lookedUpRepo,
    newRemoteProjectBlocker,
    noteTransferCandidates,
    noteTransferLabels,
    noteTransferTarget,
    repoAddedHint,
    transferFailedAfterCreate,
    type NoteRepo,
    type NoteRepoLookup,
    type NoteTransferMode,
    type NoteTransferSelection,
  } from './noteTransferTarget';
  import type { Project, ProjectLocation } from '../../types';

  interface Props {
    open: boolean;
    mode: NoteTransferMode;
    noteTitle: string;
    /** The project the note is in now; left off the list for a move. */
    currentProjectId?: string | null;
    /**
     * Looks up the repo the stored note is bound to (`null` for none), for the
     * "added if missing" hint and the repo a new remote project is created
     * with. Runs on each open; without it the note has no repo.
     */
    loadNoteRepo?: () => Promise<NoteRepo | null>;
    /**
     * The checks and saves that can refuse the transfer on their own. Runs on
     * confirm before a new project is created, so a rejection (shown inline)
     * creates nothing.
     */
    onPrepare?: () => Promise<void>;
    /** Perform the transfer into an existing (or just-created) project. */
    onConfirm: (targetProjectId: string) => Promise<void>;
  }

  let {
    open = $bindable(false),
    mode,
    noteTitle,
    currentProjectId = null,
    loadNoteRepo,
    onPrepare,
    onConfirm,
  }: Props = $props();

  const searchId = `note-transfer-search-${++inputCounter}`;
  let query = $state('');
  let selection = $state<NoteTransferSelection | null>(null);
  let location = $state<ProjectLocation>(preferences.newProjectLocation);
  let error = $state<string | null>(null);
  let working = $state(false);
  let searchElement = $state<HTMLInputElement | null>(null);
  let noteRepo = $state<NoteRepoLookup>({ status: 'resolved', repo: null });
  let wasOpen = false;
  // Bumped per lookup, so one from an earlier open that lands late is dropped.
  let noteRepoLookupId = 0;
  // The default branch for a new remote project, fetched once per open from
  // the moment the location picker shows Remote (as the new-project form
  // prefetches when a repo is chosen) and awaited on confirm, so creating
  // doesn't wait on GitHub. A transfer into an existing project never fetches.
  let defaultBranchPrefetch: { githubRepo: string; branch: Promise<string | undefined> } | null =
    null;

  $effect(() => {
    if (open && !wasOpen) {
      query = '';
      selection = null;
      location = preferences.newProjectLocation;
      error = null;
      working = false;
      defaultBranchPrefetch = null;
      lookUpNoteRepo();
      void projectsDataStore.ensureProjectsHydrated();
      void tick().then(() => focusAfterDialogEnter(searchElement));
    }
    wasOpen = open;
  });

  let labels = $derived(noteTransferLabels(mode));
  let repo = $derived(lookedUpRepo(noteRepo));
  let hint = $derived(repoAddedHint(repo));
  let candidates = $derived(
    noteTransferCandidates(projectsDataStore.projects, currentProjectId, mode)
  );
  let filtered = $derived(filterMoveTargets(candidates, projectsDataStore.reposByProject, query));
  // Checked against every project, not just the candidates: for a move the
  // current project is off the list but still exists.
  let createName = $derived(createProjectOption(query, projectsDataStore.projects));
  // A selection the query has since filtered away is no selection; the
  // "create" row tracks the query itself, so it survives typing.
  let selected = $derived.by<NoteTransferSelection | null>(() => {
    const current = selection;
    if (!current) return null;
    if (current.kind === 'new') {
      return createName ? { kind: 'new', name: createName } : null;
    }
    return filtered.some((p) => p.id === current.projectId) ? current : null;
  });
  // What confirming acts on, picked or not: it is the row shown as chosen, and
  // when it is a project to create, its location is on screen to change.
  let target = $derived(noteTransferTarget(selected, query, filtered, createName));
  let remoteProjectsAvailable = $derived(sqState.loaded && sqState.available);
  let remoteBlocker = $derived(newRemoteProjectBlocker(noteRepo));
  let remoteSelectable = $derived(remoteProjectsAvailable && !remoteBlocker);
  // The picker shows this, so a remembered Remote doesn't sit selected on a
  // disabled tile while it is blocked; it applies once the blocker clears,
  // unless the Local shown meanwhile was clicked (which commits it).
  let effectiveLocation = $derived<ProjectLocation>(remoteSelectable ? location : 'local');
  $effect(() => {
    if (open && target?.kind === 'new' && effectiveLocation === 'remote' && repo) {
      void defaultBranchFor(repo.githubRepo);
    }
  });
  let highlightedIndex = $derived.by(() => {
    const current = target;
    return current?.kind === 'existing'
      ? filtered.findIndex((p) => p.id === current.projectId)
      : -1;
  });

  function errorMessage(e: unknown): string {
    if (e instanceof Error) return e.message;
    return String(e);
  }

  function requestClose() {
    if (working) return;
    open = false;
  }

  function selectProject(project: Project) {
    selection = { kind: 'existing', projectId: project.id };
    error = null;
  }

  function lookUpNoteRepo() {
    const lookupId = ++noteRepoLookupId;
    if (!loadNoteRepo) {
      noteRepo = { status: 'resolved', repo: null };
      return;
    }
    noteRepo = { status: 'loading' };
    loadNoteRepo().then(
      (found) => {
        if (lookupId === noteRepoLookupId) noteRepo = { status: 'resolved', repo: found };
      },
      () => {
        if (lookupId === noteRepoLookupId) noteRepo = { status: 'failed' };
      }
    );
  }

  function selectCreate() {
    if (!createName) return;
    selection = { kind: 'new', name: createName };
    error = null;
  }

  /**
   * The prefetched default branch for `githubRepo`, starting the fetch if it
   * hasn't been. A failed fetch resolves to `undefined` and `create_project`
   * resolves the branch itself, as it does for the new-project form.
   */
  function defaultBranchFor(githubRepo: string): Promise<string | undefined> {
    if (defaultBranchPrefetch?.githubRepo !== githubRepo) {
      defaultBranchPrefetch = {
        githubRepo,
        branch: commands.detectDefaultBranch(githubRepo).catch(() => undefined),
      };
    }
    return defaultBranchPrefetch.branch;
  }

  /**
   * Create `name` at `at`, remembering that location only when `remember`
   * (Remote was on offer): otherwise Local was the only option, not a choice.
   */
  async function createTargetProject(
    name: string,
    at: ProjectLocation,
    remember: boolean
  ): Promise<Project> {
    let project: Project;
    if (at === 'remote' && repo) {
      // Its workspace is provisioned from the repo it is created with; the
      // transfer then finds that repo tracked and adds nothing.
      const defaultBranch = await defaultBranchFor(repo.githubRepo);
      project = await commands.createProject(
        name,
        'remote',
        repo.githubRepo,
        repo.subpath ?? undefined,
        undefined,
        undefined,
        defaultBranch
      );
    } else {
      project = await commands.createProject(name, 'local');
    }
    if (remember) setNewProjectLocation(at);
    return project;
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
    const chosen = target;
    if (working || !chosen) return;
    // The location as the picker shows it now. `onPrepare` can await the same
    // repo lookup the picker is waiting on, and that landing would otherwise
    // turn the Local on screen into a remembered Remote.
    const newLocation = effectiveLocation;
    const rememberLocation = remoteSelectable;

    working = true;
    error = null;
    let created: Project | null = null;
    try {
      await onPrepare?.();
      let projectId: string;
      if (chosen.kind === 'new') {
        created = await createTargetProject(chosen.name, newLocation, rememberLocation);
        projectsDataStore.projectCreated(created);
        projectId = created.id;
        // Keep the created project selected so a transfer failure retries
        // into it rather than creating another.
        selection = { kind: 'existing', projectId };
        query = '';
      } else {
        projectId = chosen.projectId;
      }
      await onConfirm(projectId);
      open = false;
    } catch (e) {
      error = created
        ? transferFailedAfterCreate(mode, created.name, errorMessage(e))
        : errorMessage(e);
    } finally {
      working = false;
    }
  }
</script>

<Dialog.Root bind:open={() => open, (nextOpen) => (nextOpen ? (open = true) : requestClose())}>
  <Dialog.Content
    class="sm:max-w-[480px] p-0 gap-0 overflow-hidden flex flex-col"
    showCloseButton={false}
  >
    <header class="modal-header">
      <Dialog.Title class="text-[var(--size-sm)] font-semibold text-foreground">
        {labels.title}
      </Dialog.Title>
      <Button
        variant="ghost"
        size="icon"
        class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[768px]:size-10 [&_svg]:!size-[18px]"
        title="Close"
        aria-label="Close"
        onclick={requestClose}
        disabled={working}
      >
        <X size={18} />
      </Button>
    </header>

    <form class="modal-body" onsubmit={handleSubmit}>
      <Dialog.Description class="text-[var(--size-sm)]">
        <span class="note-title">{noteTitle || 'This note'}</span>
        {#if hint}
          — {hint}
        {/if}
      </Dialog.Description>

      <div class="search-field">
        <Search size={14} class="search-icon" />
        <Input
          id={searchId}
          bind:ref={searchElement}
          bind:value={query}
          disabled={working}
          placeholder="Search projects or type a new name…"
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
            class:selected={target?.kind === 'existing' && target.projectId === project.id}
            role="radio"
            aria-checked={target?.kind === 'existing' && target.projectId === project.id}
            disabled={working}
            onclick={() => selectProject(project)}
          >
            <ProjectRowContent {project} />
          </button>
        {:else}
          {#if !createName}
            <p class="empty-state">
              {#if query.trim()}
                No projects matching "{query.trim()}"
              {:else}
                No other projects yet — type a name to create one.
              {/if}
            </p>
          {/if}
        {/each}
        {#if createName}
          <button
            type="button"
            class="project-row create-row"
            class:selected={target?.kind === 'new'}
            role="radio"
            aria-checked={target?.kind === 'new'}
            disabled={working}
            onclick={selectCreate}
          >
            <span class="create-icon" aria-hidden="true"><FolderPlus size={14} /></span>
            <span class="create-label">
              Create project <strong>{createName}</strong>
            </span>
          </button>
        {/if}
      </div>

      {#if remoteProjectsAvailable && target?.kind === 'new'}
        <div class="location-field">
          <span class="field-label">New project location</span>
          <ProjectLocationPicker
            bind:value={() => effectiveLocation, (next) => (location = next)}
            orientation="horizontal"
            disabled={working}
            remoteDisabledReason={remoteBlocker}
          />
        </div>
      {/if}

      {#if error}
        <p class="transfer-error" role="alert">{error}</p>
      {/if}

      <div class="form-actions">
        <Button
          type="button"
          variant="outline"
          class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
          onclick={requestClose}
          disabled={working}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          variant="accent"
          class="gap-1.5 px-4 py-2 text-sm max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
          disabled={!target || working}
        >
          {#if working}
            <Spinner size={14} />
            <span>{labels.busy}</span>
          {:else}
            {labels.confirm}
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

  .note-title {
    font-weight: 600;
    color: var(--text-primary);
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

  .create-row {
    gap: 10px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .create-icon {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 5px;
    flex-shrink: 0;
    color: var(--text-faint);
    background: var(--bg-hover);
  }

  .create-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .create-label strong {
    color: var(--text-primary);
    font-weight: 600;
  }

  .location-field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field-label {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .empty-state {
    margin: 0;
    padding: 12px 10px;
    color: var(--text-muted);
    font-size: var(--size-xs);
  }

  .transfer-error {
    margin: 0;
    color: var(--ui-danger);
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

    .project-list {
      max-height: none;
      flex: 1;
      min-height: 0;
    }
  }
</style>
