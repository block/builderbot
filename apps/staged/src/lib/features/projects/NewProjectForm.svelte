<!--
  NewProjectForm.svelte - Reusable project creation form

  Contains the form fields, state, and logic for creating a new project.
  Used inside NewProjectModal (as a dialog) and SplashScreen (inline).
-->
<script lang="ts">
  import { slide } from 'svelte/transition';
  import Monitor from '@lucide/svelte/icons/monitor';
  import Cloud from '@lucide/svelte/icons/cloud';
  import type { Project } from '../../types';
  import * as commands from '../../api/commands';
  import { Input } from '$lib/components/ui/input';
  import { Button } from '$lib/components/ui/button';
  import { Label } from '$lib/components/ui/label';
  import * as ToggleGroup from '$lib/components/ui/toggle-group';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoConfigForm from './RepoConfigForm.svelte';
  import type { BranchSelection } from './RepoConfigForm.svelte';
  import { parseGitHubUrl } from '../../shared/githubUrl';
  import type { RepoSelection } from '../../shared/githubUrl';
  import type { PullRequest } from '../../types';
  import { sqState } from '../settings/sq.svelte';

  interface Props {
    onCreated: (project: Project) => void;
    onCancel?: () => void;
    name?: string;
    location?: 'local' | 'remote';
    selectedRepo?: string | null;
    subpath?: string;
  }

  let {
    onCreated,
    onCancel,
    name = $bindable(''),
    location = $bindable('local'),
    selectedRepo = $bindable(null),
    subpath = $bindable(''),
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

  let remoteProjectsAvailable = $derived(sqState.loaded && sqState.available);
  let effectiveLocation = $derived(remoteProjectsAvailable ? location : 'local');

  $effect(() => {
    if (sqState.loaded && !sqState.available && location === 'remote') {
      location = 'local';
    }
  });

  let repoMissing = $derived(effectiveLocation === 'remote' && !selectedRepo);
  let canCreate = $derived(!!name.trim() && !saving && !repoMissing);

  async function handleCreate() {
    if (!canCreate) return;

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
        effectiveLocation,
        selectedRepo ?? undefined,
        normalizedSubpath,
        normalizedBranch,
        prNumber,
        matchedPr?.baseRef ?? defaultBranch ?? undefined,
        headRepo ?? undefined
      );
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

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      const target = e.target as HTMLElement;
      if (target.closest('.repo-search-wrapper')) return;
      // Don't submit if a suggestion is highlighted in the subpath dropdown
      if (target.closest('.subpath-input-wrapper')) return;
      // Don't submit if a suggestion is highlighted in the branch picker dropdown
      if (target.closest('.branch-picker-wrapper')) return;
      e.preventDefault();
      handleCreate();
    }
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

  let nameInputRef = $state<HTMLInputElement | null>(null);
  $effect(() => {
    nameInputRef?.focus();
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

<svelte:window onkeydown={handleKeydown} />

<div class="new-project-form">
  <div class="form-group">
    <Label for="project-name" class="text-muted-foreground text-xs">Name</Label>
    <Input
      bind:value={name}
      id="project-name"
      placeholder="e.g., Add dark mode, Fix login bug"
      disabled={saving}
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      oninput={handleNameInput}
      class="min-h-[42px] rounded-[10px] bg-background px-3.5 py-2.5 text-base"
      bind:ref={nameInputRef}
    />
  </div>

  {#if remoteProjectsAvailable}
    <div class="form-group">
      <div class="field-label">Location</div>
      <ToggleGroup.Root
        type="single"
        orientation="vertical"
        bind:value={location}
        disabled={saving}
        spacing={2}
        class="w-full"
      >
        {#each [{ value: 'local', label: 'Local', description: 'Run agents on your machine', icon: Monitor }, { value: 'remote', label: 'Remote', description: 'Run agents in the cloud', icon: Cloud }] as option}
          <ToggleGroup.Item
            value={option.value}
            class="flex h-auto w-full items-center justify-start gap-3 rounded-[10px] border-[1.5px] border-border bg-background px-4 py-3.5 text-left text-muted-foreground hover:border-ring hover:bg-background hover:text-foreground data-[state=on]:border-foreground data-[state=on]:bg-foreground data-[state=on]:text-background"
          >
            <option.icon size={22} class="shrink-0" />
            <span class="flex min-w-0 flex-col gap-0.5">
              <span class="text-sm font-medium">{option.label}</span>
              <span class="text-xs opacity-70">{option.description}</span>
            </span>
          </ToggleGroup.Item>
        {/each}
      </ToggleGroup.Root>
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
    repoRequired={effectiveLocation === 'remote'}
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
      variant="outline"
      class={!onCancel ? 'w-full' : ''}
      onclick={handleCreate}
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

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
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
