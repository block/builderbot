<!--
  NewProjectForm.svelte - Reusable project creation form

  Contains the form fields, state, and logic for creating a new project.
  Used inside NewProjectModal (as a dialog) and SplashScreen (inline).

  The location starts on the one the last new project was created with
  (`preferences.newProjectLocation`, shared with the note transfer picker),
  and creating a project with the picker on screen remembers its choice.
-->
<script lang="ts">
  import { slide } from 'svelte/transition';
  import type { Project } from '../../types';
  import * as commands from '../../api/commands';
  import { Input } from '$lib/components/ui/input';
  import { Button } from '$lib/components/ui/button';
  import { Label } from '$lib/components/ui/label';
  import Spinner from '../../shared/Spinner.svelte';
  import ProjectLocationPicker from './ProjectLocationPicker.svelte';
  import RepoConfigForm from './RepoConfigForm.svelte';
  import type { BranchSelection } from './RepoConfigForm.svelte';
  import { parseGitHubUrl } from '../../shared/githubUrl';
  import type { RepoSelection } from '../../shared/githubUrl';
  import { autofocusOnOpen } from '../../shared/focusAfterDialogEnter';
  import {
    findEnabledSubmitButton,
    isDefaultActionKeydown,
  } from '../../shared/dialogDefaultAction';
  import type { PullRequest } from '../../types';
  import { bloxAvailable, sqState } from '../settings/sq.svelte';
  import { preferences, setNewProjectLocation } from '../settings/preferences.svelte';

  interface Props {
    onCreated: (project: Project) => void;
    onCancel?: () => void;
    name?: string;
    location?: 'local' | 'remote';
    selectedRepo?: string | null;
    subpath?: string;
    /** Preselect this repo (and subpath) when the form opens. */
    initialRepo?: RepoSelection | null;
  }

  let {
    onCreated,
    onCancel,
    name = $bindable(''),
    location = $bindable(preferences.newProjectLocation),
    selectedRepo = $bindable(null),
    subpath = $bindable(''),
    initialRepo = null,
  }: Props = $props();

  let branchName = $state('');
  let isNewBranch = $state(false);
  let matchedPr = $state<PullRequest | null>(null);
  let defaultBranch = $state<string | null>(null);
  let headRepo = $state<string | null>(null);

  let saving = $state(false);
  let error = $state<string | null>(null);
  let repoConfigApi = $state<
    | {
        waitForSubpathValidation: () => Promise<{ valid: boolean; error?: string }>;
        selectRepo: (selection: RepoSelection) => void;
        reset: () => void;
      }
    | undefined
  >(undefined);

  // Clear error when user edits the subpath
  $effect(() => {
    subpath;
    error = null;
  });

  // Apply the initial repo through the same selectRepo path a pasted GitHub
  // URL takes, once the RepoConfigForm exposes its API after mount.
  let initialRepoApplied = false;
  $effect(() => {
    if (initialRepoApplied || !initialRepo || !repoConfigApi) return;
    initialRepoApplied = true;
    repoConfigApi.selectRepo(initialRepo);
  });

  let remoteProjectsAvailable = $derived(bloxAvailable());
  let effectiveLocation = $derived(remoteProjectsAvailable ? location : 'local');

  $effect(() => {
    if (sqState.loaded && !remoteProjectsAvailable && location === 'remote') {
      location = 'local';
    }
  });

  // A remote project needs no repo: its workspace is provisioned when the
  // first repo is added.
  let canCreate = $derived(!!name.trim() && !saving);

  async function handleCreate() {
    if (!canCreate) return;
    // The location as submitted: the `sq` check landing during the subpath
    // validation below must not turn a create made with no picker on screen
    // into a remote one, nor remember a location that wasn't created with.
    const createLocation = effectiveLocation;
    // Only a location picked with Remote on offer is a choice to remember.
    const rememberLocation = remoteProjectsAvailable;

    saving = true;
    error = null;

    try {
      // If there's a subpath and a repo, validate before creating
      if (selectedRepo && subpath.trim() && repoConfigApi) {
        const validation = await repoConfigApi.waitForSubpathValidation();
        if (!validation.valid) {
          error = validation.error ?? 'Invalid path in repo';
          saving = false;
          return;
        }
      }

      const normalizedSubpath = selectedRepo
        ? subpath.trim().replace(/^\/+|\/+$/g, '') || undefined
        : undefined;

      const normalizedBranch = selectedRepo ? branchName.trim() || undefined : undefined;
      const prNumber = matchedPr?.number ?? undefined;

      const project = await commands.createProject(
        name.trim(),
        createLocation,
        selectedRepo ?? undefined,
        normalizedSubpath,
        normalizedBranch,
        prNumber,
        matchedPr?.baseRef ?? defaultBranch ?? undefined,
        headRepo ?? undefined
      );
      if (rememberLocation) setNewProjectLocation(createLocation);
      if (matchedPr) {
        const noteTitle = `PR #${matchedPr.number}: ${matchedPr.title}`;
        await commands.createProjectNote(project.id, noteTitle, matchedPr.body ?? '');
      }
      onCreated(project);
    } catch (e) {
      if (typeof e === 'string') {
        error = e;
      } else if (e instanceof Error) {
        error = e.message;
      } else {
        error = String(e);
      }
      saving = false;
    }
  }

  // Return creates the project through a form that owns just the Name field
  // and Create (tied to it by its `form` attribute). In the Name field the
  // browser submits it. The repo, subpath and branch fields are outside it:
  // Return there picks from their dropdowns and never creates the project
  // half filled in. The repo chips are outside it too, or the first of them,
  // a button with no `type`, would be the button a Return presses.
  const formId = $props.id();
  let formEl = $state<HTMLFormElement | null>(null);

  function handleSubmit(e: SubmitEvent) {
    e.preventDefault();
    void handleCreate();
  }

  // A Return on the fields' container presses Create, as one on a dialog
  // submits its form (see dialog-content). WebKit gives a clicked button no
  // focus; it goes to the nearest focusable ancestor, which `tabindex="-1"`
  // makes this container. So a Return after a click on a repo chip or a
  // location tile still creates the project, on the splash screen too, where
  // there is no dialog around the form to hear it. A focused Cancel or
  // location tile takes its own Return.
  function handleKeydown(e: KeyboardEvent) {
    if (!formEl || !isDefaultActionKeydown(e)) return;
    const create = findEnabledSubmitButton(formEl);
    if (!create) return;
    e.preventDefault();
    e.stopPropagation();
    create.click();
  }

  /** Derive a human-friendly project name from a branch name like "feat/dark-mode". */
  function nameFromBranch(branch: string): string {
    const last = branch.split('/').pop() ?? branch;
    if (!last) return branch;
    const spaced = last.replace(/[-_]/g, ' ');
    return spaced.charAt(0).toUpperCase() + spaced.slice(1);
  }

  function handleBranchSelected(selection: BranchSelection) {
    if (!name.trim()) {
      name = selection.kind === 'pr' ? selection.label : nameFromBranch(selection.branchName);
    }
  }

  // On an iPhone or iPad the name field waits for a tap (see autofocusOnOpen).
  let nameInputRef = $state<HTMLInputElement | null>(null);
  $effect(() => {
    autofocusOnOpen(nameInputRef);
  });

  function handleNameInput() {
    const parsed = parseGitHubUrl(name);
    if (parsed) {
      name = '';
      if (repoConfigApi) {
        repoConfigApi.selectRepo(parsed);
      }
      // Blur the name field so focus doesn't jump into the newly-revealed BranchPicker
      (document.activeElement as HTMLElement | null)?.blur();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="new-project-form" tabindex="-1" onkeydown={handleKeydown}>
  <form class="form-group" id={formId} bind:this={formEl} onsubmit={handleSubmit}>
    <Label for="project-name" class="text-muted-foreground text-xs">Name</Label>
    <Input
      bind:value={name}
      id="project-name"
      placeholder="e.g., Add dark mode, Fix login bug"
      disabled={saving}
      autocomplete="off"
      autocorrect="off"
      autocapitalize="sentences"
      spellcheck={false}
      oninput={handleNameInput}
      class="min-h-[42px] rounded-[10px] bg-background px-3.5 py-2.5"
      bind:ref={nameInputRef}
    />
  </form>

  {#if remoteProjectsAvailable}
    <div class="form-group">
      <div class="field-label">Location</div>
      <ProjectLocationPicker bind:value={location} disabled={saving} />
    </div>
  {/if}

  <RepoConfigForm
    bind:selectedRepo
    bind:headRepo
    bind:subpath
    bind:branchName
    bind:isNewBranch
    bind:matchedPr
    bind:defaultBranch
    bind:api={repoConfigApi}
    disabled={saving}
    onBranchSelected={handleBranchSelected}
  />

  {#if error}
    <div class="error-message" transition:slide={{ duration: 150 }}>{error}</div>
  {/if}

  <div class="actions">
    {#if onCancel}
      <Button variant="ghost" onclick={onCancel} disabled={saving}>Cancel</Button>
    {/if}
    <Button
      type="submit"
      form={formId}
      variant="outline"
      class={!onCancel ? 'w-full' : ''}
      disabled={!canCreate}
    >
      {#if saving}
        <span class="inline-flex items-center gap-1.5">
          <Spinner size={14} />
          <span>Creating...</span>
        </span>
      {:else}
        Create Project
      {/if}
    </Button>
  </div>
</div>

<style>
  .new-project-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
    /* Focusable only to hear Return (see handleKeydown); it shows no ring. */
    outline: none;
  }

  /* In a full-screen dialog (NewProjectModal on a phone) the form is the
     dialog's scrolling row: the on-screen keyboard shrinks the dialog to
     what is visible above it, and the rows that no longer fit (the repo
     chips, Create, Cancel) are a scroll away instead of under the keyboard.
     The dialog's grid gives this row the height left under the header
     (`mobile:grid-rows-[auto_minmax(0,1fr)]`). Elsewhere (the splash
     screen, the desktop dialog) the form stays content-sized. The repo
     dropdown is portaled out of the form (RepoSearchInput), so the scroll
     clip never cuts it off. */
  :global([data-full-screen]) .new-project-form {
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field-label {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .error-message {
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  /* Last row of NewProjectModal; clear of the home indicator when that is
     full screen (`--dialog-safe-bottom`, see dialog-content). 0 elsewhere. */
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding-bottom: var(--dialog-safe-bottom, 0px);
  }

  @media (max-width: 640px) {
    .actions {
      flex-direction: column-reverse;
    }

    .actions :global(button) {
      width: 100%;
      min-height: 44px;
    }
  }
</style>
