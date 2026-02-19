<!--
  NewProjectForm.svelte - Reusable project creation form

  Contains the form fields, state, and logic for creating a new project.
  Used inside NewProjectModal (as a dialog) and SplashScreen (inline).

  The repo picker slides in from the right using a svelte/motion spring
  rather than opening as a separate overlay modal.
-->
<script lang="ts">
  import { GitBranch, Plus, Monitor, Cloud, Command } from 'lucide-svelte';
  import { spring } from 'svelte/motion';
  import type { Project, RecentRepo } from '../../types';
  import * as commands from '../../commands';
  import FormInput from '../../shared/FormInput.svelte';
  import FormButton from '../../shared/FormButton.svelte';
  import FormToggle from '../../shared/FormToggle.svelte';
  import GitHubRepoPicker from './GitHubRepoPicker.svelte';
  import { onMount } from 'svelte';

  interface Props {
    onCreated: (project: Project) => void;
    onCancel?: () => void;
  }

  let { onCreated, onCancel }: Props = $props();

  let name = $state('');
  let location = $state<'local' | 'remote'>('local');
  let selectedRepo = $state<string | null>(null);
  let subpath = $state('');
  let saving = $state(false);
  let error = $state<string | null>(null);
  let showRepoPicker = $state(false);
  let recentRepos = $state<RecentRepo[]>([]);
  let isMonorepo = $state(false);
  let checkingMonorepo = $state(false);

  // Slide animation state
  let pickerEverShown = $state(false);
  let formHeight = $state(0);
  let pickerHeight = $state(0);
  let heightInitialized = $state(false);
  let pickerRef: GitHubRepoPicker | undefined = $state();

  const slideX = spring(0, { stiffness: 0.15, damping: 0.78 });
  const heightSpring = spring(0, { stiffness: 0.15, damping: 0.78 });

  $effect(() => {
    if (showRepoPicker) pickerEverShown = true;
  });

  $effect(() => {
    slideX.set(showRepoPicker ? -100 : 0);
  });

  $effect(() => {
    const target = showRepoPicker ? pickerHeight : formHeight;
    if (target > 0) {
      if (!heightInitialized) {
        heightSpring.set(target, { hard: true });
        heightInitialized = true;
      } else {
        heightSpring.set(target);
      }
    }
  });

  $effect(() => {
    if (showRepoPicker && pickerRef) {
      const timer = setTimeout(() => pickerRef?.focusSearch(), 80);
      return () => clearTimeout(timer);
    }
  });

  onMount(async () => {
    try {
      recentRepos = await commands.listRecentRepos(3);
    } catch (e) {
      // Fail silently - recent repos are optional
    }
  });

  async function checkIfMonorepo(repo: string) {
    if (!repo) {
      isMonorepo = false;
      return;
    }

    checkingMonorepo = true;
    try {
      const moduleCount = await commands.checkMonorepoModules(repo);
      isMonorepo = moduleCount >= 20;
    } catch (e) {
      isMonorepo = false;
    } finally {
      checkingMonorepo = false;
    }
  }

  $effect(() => {
    if (selectedRepo) {
      checkIfMonorepo(selectedRepo);
    } else {
      isMonorepo = false;
    }
  });

  async function handleCreate() {
    if (!name.trim() || saving) return;

    saving = true;
    error = null;

    try {
      const normalizedSubpath = selectedRepo
        ? subpath.trim().replace(/^\/+|\/+$/g, '') || undefined
        : undefined;

      const project = await commands.createProject(
        name.trim(),
        location,
        selectedRepo ?? undefined,
        normalizedSubpath
      );
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
    if (showRepoPicker) return;

    if (e.key === 'Enter') {
      e.preventDefault();
      handleCreate();
    } else if (recentRepos.length > 0 && !selectedRepo) {
      const num = parseInt(e.key);
      if (num >= 1 && num <= recentRepos.length) {
        e.preventDefault();
        const recent = recentRepos[num - 1];
        selectedRepo = recent.githubRepo;
        if (recent.subpath) {
          subpath = recent.subpath;
        }
      }
    }
  }

  function handleRepoSelected(nameWithOwner: string, selectedSubpath?: string) {
    selectedRepo = nameWithOwner;
    if (selectedSubpath) {
      subpath = selectedSubpath;
    }
    showRepoPicker = false;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="slide-container" style:height="{$heightSpring}px">
  <div class="slide-track" style:transform="translateX({$slideX}%)">
    <div class="slide-panel" bind:clientHeight={formHeight} inert={showRepoPicker || undefined}>
      <div class="new-project-form">
        <div class="form-group">
          <label for="project-name">Name</label>
          <FormInput
            bind:value={name}
            id="project-name"
            placeholder="e.g., Add dark mode, Fix login bug"
            disabled={saving}
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck={false}
            autofocus
          />
        </div>

        <div class="form-group">
          <div class="field-label">Location</div>
          <FormToggle
            bind:value={location}
            options={[
              {
                value: 'local',
                label: 'Local',
                description: 'Run agents on your machine',
                icon: Monitor,
              },
              {
                value: 'remote',
                label: 'Remote',
                description: 'Run agents in the cloud',
                icon: Cloud,
              },
            ]}
            disabled={saving}
          />
        </div>

        <div class="form-group">
          <label for="project-repo-select">Repository</label>
          {#if selectedRepo}
            <div class="repo-info">
              <GitBranch size={14} class="repo-info-icon" />
              <div class="repo-details">
                <span class="repo-name">{selectedRepo}</span>
              </div>
              <button class="change-button" onclick={() => (showRepoPicker = true)}>Change</button>
              <button class="change-button" onclick={() => (selectedRepo = null)}>Clear</button>
            </div>
          {:else}
            <div class="repo-picker-wrapper">
              <FormButton
                variant="secondary"
                class="select-repo-button"
                onclick={() => (showRepoPicker = true)}
              >
                Select repository
              </FormButton>
              {#if recentRepos.length > 0}
                <div class="recent-repos-section">
                  {#each recentRepos as recent, i}
                    <FormButton
                      variant="ghost"
                      class="recent-repo-btn"
                      onclick={() => {
                        selectedRepo = recent.githubRepo;
                        if (recent.subpath) {
                          subpath = recent.subpath;
                        }
                      }}
                    >
                      <GitBranch size={12} />
                      <span class="recent-repo-name">
                        {recent.githubRepo}{#if recent.subpath}<span class="recent-repo-subpath"
                            >/{recent.subpath}</span
                          >{/if}
                      </span>
                      <span class="keyboard-shortcut">
                        <Command size={10} />
                        {i + 1}
                      </span>
                      <Plus size={12} />
                    </FormButton>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
        </div>

        {#if selectedRepo}
          <div class="form-group">
            <label for="project-subpath"
              >Subpath
              <span class="field-badge {isMonorepo ? 'recommended' : 'optional'}"
                >{isMonorepo ? 'Recommended' : 'Optional'}</span
              ></label
            >
            <FormInput
              bind:value={subpath}
              id="project-subpath"
              placeholder="e.g., packages/frontend"
              disabled={saving}
              autocomplete="off"
              autocorrect="off"
              autocapitalize="off"
              spellcheck={false}
            />
          </div>
        {/if}

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="actions">
          {#if onCancel}
            <FormButton onclick={onCancel} disabled={saving}>Cancel</FormButton>
          {/if}
          <FormButton
            variant="primary"
            class={!onCancel ? 'full-width-btn' : ''}
            onclick={handleCreate}
            disabled={saving || !name.trim()}
          >
            {saving ? 'Creating...' : 'Create Project'}
          </FormButton>
        </div>
      </div>
    </div>

    <div
      class="slide-panel picker-panel"
      bind:clientHeight={pickerHeight}
      inert={!showRepoPicker || undefined}
    >
      {#if pickerEverShown}
        <GitHubRepoPicker
          bind:this={pickerRef}
          onSelect={handleRepoSelected}
          onBack={() => (showRepoPicker = false)}
        />
      {/if}
    </div>
  </div>
</div>

<style>
  .slide-container {
    overflow: hidden;
    position: relative;
  }

  .slide-track {
    display: flex;
    will-change: transform;
  }

  .slide-panel {
    width: 100%;
    flex-shrink: 0;
  }

  .picker-panel {
    height: 380px;
  }

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

  label {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .field-label {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .field-badge {
    display: inline-block;
    font-size: 9px;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 3px;
    margin-left: 6px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .field-badge.optional {
    background-color: var(--bg-hover);
    color: var(--text-faint);
  }

  .field-badge.recommended {
    background-color: var(--ui-accent);
    color: var(--bg-deepest);
  }

  .repo-picker-wrapper {
    overflow: hidden;
  }

  :global(.select-repo-button) {
    width: 100%;
    justify-content: flex-start;
  }

  .repo-info {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 42px;
    padding: 10px 14px;
    background: var(--text-primary);
    color: var(--bg-deepest);
    border: 1.5px solid var(--text-primary);
    border-radius: 10px;
  }

  .repo-info :global(.repo-info-icon) {
    color: var(--bg-deepest);
  }

  .repo-details {
    min-width: 0;
    flex: 1;
  }

  .repo-name {
    font-size: var(--size-sm);
    color: var(--bg-deepest);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .change-button {
    border: 1px solid var(--bg-chrome);
    border-radius: 7px;
    background: transparent;
    color: var(--bg-chrome);
    padding: 6px 10px;
    cursor: pointer;
    transition:
      border-color 0.15s ease,
      color 0.15s ease;
  }

  .change-button:hover {
    color: var(--bg-deepest);
    border-color: var(--bg-deepest);
  }

  .recent-repos-section {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 0 4px;
    margin-top: 2px;
  }

  :global(.recent-repo-btn) {
    width: 100%;
    justify-content: flex-start;
    min-height: 28px !important;
    padding: 4px 8px !important;
    font-size: var(--size-xs) !important;
    border-radius: 6px !important;
  }

  .recent-repo-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
  }

  .recent-repo-subpath {
    color: var(--text-faint);
  }

  .keyboard-shortcut {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px 5px;
    background: var(--bg-primary);
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    flex-shrink: 0;
    line-height: 1;
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

  :global(.full-width-btn) {
    width: 100%;
  }
</style>
