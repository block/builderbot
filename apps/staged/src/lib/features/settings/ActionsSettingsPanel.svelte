<script lang="ts">
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import { cubicInOut } from 'svelte/easing';
  import FolderGit2 from '@lucide/svelte/icons/folder-git-2';
  import Play from '@lucide/svelte/icons/play';
  import Zap from '@lucide/svelte/icons/zap';
  import Pin from '@lucide/svelte/icons/pin';
  import Plus from '@lucide/svelte/icons/plus';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import Save from '@lucide/svelte/icons/save';
  import Pencil from '@lucide/svelte/icons/pencil';
  import Code2 from '@lucide/svelte/icons/code-2';
  import Search from '@lucide/svelte/icons/search';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import * as Select from '$lib/components/ui/select';
  import * as AlertDialog from '$lib/components/ui/alert-dialog';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { Label } from '$lib/components/ui/label';
  import type { ActionContext, ProjectAction } from '../../api/commands';
  import * as commands from '../../api/commands';
  import {
    detectRepoActions,
    listenToRepoActionsDetection,
    type ActionType,
  } from '../actions/actions';
  import ActionIcon from '../actions/ActionIcon.svelte';
  import IconPicker from '../actions/IconPicker.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import { hueSliderGradient } from '../../shared/badgeColors';
  import { matchesRepoSearch } from './repoContextSearch';
  import { toast } from 'svelte-sonner';
  import { getPreferredAgent } from './preferences.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { navigation } from '../layout/navigation.svelte';
  import { consumeRepoSettingsTarget } from './repoSettingsTarget';

  type RepoAttachment = {
    projectId: string;
    projectName: string;
    projectRepoId: string;
    branchName: string;
  };

  /** A repo entry from either an action context, a badge, or both. */
  type RepoEntry = {
    key: string;
    githubRepo: string;
    subpath: string;
    context: ActionContext | null;
  };

  let contexts = $state<ActionContext[]>([]);
  let selectedRepoKey = $state<string | null>(null);
  let loadingContexts = $state(false);
  let loadingRepoAttachments = $state(false);
  let repoAttachmentsByContext = $state<Record<string, RepoAttachment[]>>({});
  let repoAttachmentLoadGeneration = 0;
  let repoSearch = $state('');
  let sidebarEl: HTMLElement | null = null;

  let actions = $state<ProjectAction[]>([]);
  let loadingActions = $state(false);
  let detecting = $state(false);
  let deletingRepo = $state(false);
  let showDeleteAllConfirm = $state(false);
  let showDeleteRepoConfirm = $state(false);
  let editingAction = $state<ProjectAction | null>(null);
  let editForm = $state({
    name: '',
    command: '',
    actionType: 'run' as ActionType,
    autoCommit: false,
    pinned: false,
    icon: null as string | null,
  });
  let badgeEditName = $state('');
  let badgeEditHue = $state(0);
  let badgeError = $state('');

  let unlistenDetection: (() => void) | undefined;

  onMount(async () => {
    unlistenDetection = listenToRepoActionsDetection((event) => {
      const ctx = selectedContext;
      if (!ctx) return;
      if (event.githubRepo !== ctx.githubRepo || event.subpath !== (ctx.subpath ?? null)) return;
      detecting = event.detecting;
      if (!event.detecting) {
        loadActions();
      }
    });

    await repoBadgeStore.loadAll();
    await loadContexts();
    await ensureBadgesForContexts(contexts);
  });

  onDestroy(() => {
    unlistenDetection?.();
  });

  async function ensureBadgesForContexts(ctxs: ActionContext[]) {
    if (ctxs.length === 0) return;
    await repoBadgeStore.ensureForRepos(
      ctxs.map((c) => ({ githubRepo: c.githubRepo, subpath: c.subpath }))
    );
  }

  $effect(() => {
    // Only re-run when the selected entry changes, not when badge values
    // update in the store (which would clobber in-progress edits after save).
    void selectedRepoKey;
    untrack(() => {
      badgeError = '';
      const badge = selectedBadge;
      if (badge) {
        badgeEditName = badge.shortName;
        badgeEditHue = badge.hue;
      }
    });
  });

  async function saveBadge() {
    if (!selectedEntry || !badgeEditName.trim()) return;
    badgeError = '';
    try {
      await repoBadgeStore.update(
        selectedEntry.githubRepo,
        selectedEntry.subpath,
        badgeEditName.trim(),
        badgeEditHue
      );
    } catch (e) {
      const msg = typeof e === 'string' ? e : e instanceof Error ? e.message : String(e);
      badgeError = msg;
    }
  }

  function repoKey(githubRepo: string, subpath: string | null | undefined): string {
    return `${githubRepo}::${subpath ?? ''}`;
  }

  function repoDisplay(githubRepo: string, subpath: string | null | undefined): string {
    return subpath ? `${githubRepo}/${subpath}` : githubRepo;
  }

  function formatProjectCount(count: number): string {
    return `${count} project${count === 1 ? '' : 's'}`;
  }

  /** Merge action contexts and orphan badges into a single list. */
  let mergedEntries = $derived.by<RepoEntry[]>(() => {
    const entries: RepoEntry[] = contexts.map((c) => ({
      key: repoKey(c.githubRepo, c.subpath),
      githubRepo: c.githubRepo,
      subpath: c.subpath ?? '',
      context: c,
    }));

    const contextKeys = new Set(entries.map((e) => e.key));
    for (const badge of repoBadgeStore.all()) {
      const k = repoKey(badge.githubRepo, badge.subpath);
      if (!contextKeys.has(k)) {
        entries.push({
          key: k,
          githubRepo: badge.githubRepo,
          subpath: badge.subpath,
          context: null,
        });
      }
    }

    return entries;
  });

  let selectedEntry = $derived(mergedEntries.find((e) => e.key === selectedRepoKey) ?? null);
  let selectedContext = $derived(selectedEntry?.context ?? null);
  let selectedContextAttachments = $derived(
    selectedContext ? (repoAttachmentsByContext[selectedContext.id] ?? []) : []
  );
  let selectedBadge = $derived(
    selectedEntry
      ? repoBadgeStore.lookup(selectedEntry.githubRepo, selectedEntry.subpath)
      : undefined
  );

  async function loadRepoAttachments(actionContexts: ActionContext[]) {
    const generation = ++repoAttachmentLoadGeneration;
    loadingRepoAttachments = true;

    const byContext: Record<string, RepoAttachment[]> = Object.fromEntries(
      actionContexts.map((context) => [context.id, [] as RepoAttachment[]])
    );

    try {
      if (actionContexts.length === 0) {
        repoAttachmentsByContext = {};
        return;
      }

      const contextIdByRepo = new Map(
        actionContexts.map((context) => [repoKey(context.githubRepo, context.subpath), context.id])
      );
      const { data: projects } = await commands.listProjects();
      const reposByProject = await Promise.all(
        projects.map(async (project) => {
          const { data: repos } = await commands.listProjectRepos(project.id);
          return { project, repos };
        })
      );

      for (const { project, repos } of reposByProject) {
        for (const repo of repos) {
          const contextId = contextIdByRepo.get(repoKey(repo.githubRepo, repo.subpath));
          if (!contextId) continue;
          byContext[contextId] = [
            ...byContext[contextId],
            {
              projectId: project.id,
              projectName: project.name,
              projectRepoId: repo.id,
              branchName: repo.branchName,
            },
          ];
        }
      }

      if (generation === repoAttachmentLoadGeneration) {
        repoAttachmentsByContext = byContext;
      }
    } catch (e) {
      console.error('Failed to load repo attachments for action contexts:', e);
      if (generation === repoAttachmentLoadGeneration) {
        repoAttachmentsByContext = byContext;
      }
    } finally {
      if (generation === repoAttachmentLoadGeneration) {
        loadingRepoAttachments = false;
      }
    }
  }

  async function loadContexts() {
    // A repo card's "Repo Settings" action parks the repo it wants selected.
    // Take it up front so a failed load drops the request rather than leaving
    // it parked for some later, unrelated visit to Settings → Repos.
    const requested = consumeRepoSettingsTarget();
    const requestedKey = requested ? repoKey(requested.githubRepo, requested.subpath) : null;
    let revealRequested = false;

    loadingContexts = true;
    try {
      const nextContexts = await commands.listActionContexts();
      contexts = nextContexts;
      await loadRepoAttachments(nextContexts);
      // Honor the parked target now that the entry list is resolved.
      if (requestedKey && mergedEntries.some((e) => e.key === requestedKey)) {
        selectedRepoKey = requestedKey;
        revealRequested = true;
      } else if (!selectedRepoKey && mergedEntries.length > 0) {
        selectedRepoKey = mergedEntries[0].key;
      } else if (selectedRepoKey && !mergedEntries.some((e) => e.key === selectedRepoKey)) {
        selectedRepoKey = mergedEntries.length > 0 ? mergedEntries[0].key : null;
      }
      await loadActions();
    } catch (e) {
      console.error('Failed to load action contexts:', e);
    } finally {
      loadingContexts = false;
      // The sidebar renders a spinner until this flag clears, so the selected
      // row only exists to scroll to after it does.
      if (revealRequested) void revealSelectedRepo();
    }
  }

  /** Scroll the selected repo's sidebar row into view once it has rendered. */
  async function revealSelectedRepo() {
    await tick();
    sidebarEl?.querySelector('.context-item.selected')?.scrollIntoView({ block: 'nearest' });
  }

  async function loadActions() {
    if (!selectedContext) {
      actions = [];
      return;
    }
    loadingActions = true;
    try {
      actions = await commands.listRepoActions(
        selectedContext.githubRepo,
        selectedContext.subpath ?? undefined
      );
    } catch (e) {
      console.error('Failed to load actions:', e);
      actions = [];
    } finally {
      loadingActions = false;
    }
  }

  $effect(() => {
    selectedRepoKey;
    loadActions();
  });

  $effect(() => {
    detecting = selectedContext?.detectingActions ?? false;
  });

  async function detectActions() {
    if (!selectedContext) return;

    // Capture context before the async gap so that switching repo contexts
    // while detection is in-flight doesn't cause actions to be saved to
    // the wrong context.
    const entryKey = selectedRepoKey;
    const githubRepo = selectedContext.githubRepo;
    const subpath = selectedContext.subpath ?? undefined;

    detecting = true;
    try {
      const provider = getPreferredAgent(agentState.providers) ?? undefined;
      // The backend persists the suggestions it detects — inside the window it
      // reports as detecting, so the list it hands back is final and every
      // action surface reloads off the same detecting:false broadcast.
      const detected = await detectRepoActions(githubRepo, subpath, provider);

      // After the await the user may have navigated away from this context, so
      // only adopt the list while we're still viewing the one we detected for;
      // the backend writes were scoped to the captured values regardless.
      if (selectedRepoKey === entryKey) {
        actions = detected;
      }

      await loadContexts();
    } catch (e) {
      console.error('Failed to detect actions:', e);
      toast.error('Failed to detect actions', { description: String(e) });
    } finally {
      detecting = false;
    }
  }

  function startAddAction() {
    editForm = {
      name: '',
      command: '',
      actionType: 'run',
      autoCommit: false,
      pinned: false,
      icon: null,
    };
    editingAction = {} as ProjectAction;
  }

  function startEditAction(action: ProjectAction) {
    editForm = {
      name: action.name,
      command: action.command,
      actionType: action.actionType as ActionType,
      autoCommit: action.autoCommit,
      pinned: action.pinned,
      icon: action.icon,
    };
    editingAction = action;
  }

  function cancelEdit() {
    editingAction = null;
  }

  async function saveAction() {
    if (!selectedContext || !editForm.name || !editForm.command) return;

    // Capture context values before any await to avoid stale references
    // if the user switches repo contexts while the save is in-flight.
    const githubRepo = selectedContext.githubRepo;
    const subpath = selectedContext.subpath ?? undefined;
    const entryKey = selectedRepoKey;

    try {
      if (!editingAction?.id) {
        const nextSortOrder = Math.max(...actions.map((a) => a.sortOrder), 0) + 1;
        const newAction = await commands.createRepoAction(
          githubRepo,
          subpath,
          editForm.name,
          editForm.command,
          editForm.actionType,
          nextSortOrder,
          editForm.autoCommit,
          editForm.pinned,
          editForm.icon
        );
        if (selectedRepoKey === entryKey) {
          actions = [...actions, newAction];
        }
      } else {
        const actionId = editingAction.id;
        await commands.updateProjectAction(
          actionId,
          editForm.name,
          editForm.command,
          editForm.actionType,
          editingAction.sortOrder,
          editForm.autoCommit,
          editForm.pinned,
          editForm.icon
        );
        actions = actions.map((a) =>
          a.id === actionId
            ? {
                ...a,
                name: editForm.name,
                command: editForm.command,
                actionType: editForm.actionType,
                autoCommit: editForm.autoCommit,
                pinned: editForm.pinned,
                icon: editForm.icon,
              }
            : a
        );
      }

      editingAction = null;
      window.dispatchEvent(new CustomEvent('project-actions-changed'));
    } catch (e) {
      console.error('Failed to save action:', e);
    }
  }

  async function deleteAction(actionId: string) {
    try {
      await commands.deleteProjectAction(actionId);
      actions = actions.filter((a) => a.id !== actionId);
      window.dispatchEvent(new CustomEvent('project-actions-changed'));
    } catch (e) {
      console.error('Failed to delete action:', e);
    }
  }

  async function deleteAllActions() {
    if (!selectedContext) return;

    // Capture the entry key before the await so a concurrent context
    // switch doesn't clear the wrong context's actions from the UI.
    const entryKey = selectedRepoKey;
    const repoContextId = selectedContext.id;

    try {
      await commands.deleteAllRepoActions(repoContextId);
      if (selectedRepoKey === entryKey) {
        actions = [];
      }
      showDeleteAllConfirm = false;
      window.dispatchEvent(new CustomEvent('project-actions-changed'));
    } catch (e) {
      console.error('Failed to delete all actions:', e);
    }
  }

  async function deleteRepo() {
    if (!selectedEntry) return;

    const entry = selectedEntry;
    const entryKey = entry.key;

    deletingRepo = true;
    try {
      if (entry.context) {
        const contextId = entry.context.id;
        const attachments = [...(repoAttachmentsByContext[contextId] ?? [])];

        for (const attachment of attachments) {
          await commands.removeProjectRepo(attachment.projectId, attachment.projectRepoId);
        }

        await commands.deleteActionContext(contextId);
      }

      await commands.deleteRepoBadge(entry.githubRepo, entry.subpath);
      repoBadgeStore.remove(entry.githubRepo, entry.subpath);
      if (selectedRepoKey === entryKey) {
        actions = [];
      }
      showDeleteRepoConfirm = false;
      window.dispatchEvent(new CustomEvent('project-actions-changed'));
      await loadContexts();
    } catch (e) {
      console.error('Failed to delete repo:', e);
    } finally {
      deletingRepo = false;
    }
  }

  const sidebarSlideMs = 350;

  /**
   * Slides the sidebar in from the left when Repos is selected and back out
   * when it is deselected — the same S-curve motion in both directions. On
   * the way out, SettingsPage keeps this panel stacked under the incoming
   * panel until the outro finishes; the z-index lifts the sidebar above it
   * so the slide stays visible. Skipped when the whole settings view is
   * being torn down.
   */
  function sidebarSlide(node: HTMLElement) {
    const w = node.offsetWidth;
    return {
      duration: navigation.activeView === 'settings' ? sidebarSlideMs : 0,
      easing: cubicInOut,
      css: (t: number) => `margin-left: ${(t - 1) * w}px; z-index: 1;`,
    };
  }

  /** Hides everything but the sliding sidebar during the outro so the
      incoming panel shows through immediately. */
  function hideOnExit(_node: HTMLElement) {
    return {
      duration: navigation.activeView === 'settings' ? sidebarSlideMs : 0,
      css: () => 'opacity: 0; pointer-events: none;',
    };
  }

  let sortedEntries = $derived.by(() => {
    return [...mergedEntries].sort((a, b) => {
      const aDisplay = a.subpath ? `${a.githubRepo}/${a.subpath}` : a.githubRepo;
      const bDisplay = b.subpath ? `${b.githubRepo}/${b.subpath}` : b.githubRepo;
      return aDisplay.localeCompare(bDisplay);
    });
  });

  let filteredEntries = $derived.by(() => {
    const query = repoSearch.trim();
    if (!query) return sortedEntries;

    return sortedEntries.filter((entry) =>
      matchesRepoSearch(entry.githubRepo, entry.subpath, query)
    );
  });

  let groupedActions = $derived.by(() => {
    const groups: Record<string, ProjectAction[]> = {
      prerun: [],
      run: [],
      build: [],
      test: [],
      format: [],
      check: [],
      cleanUp: [],
    };
    for (const action of actions) {
      const type = action.actionType;
      if (groups[type]) groups[type].push(action);
    }
    return groups;
  });
</script>

<div class="actions-settings-panel">
  <div class="panel-body">
    <aside class="sidebar" bind:this={sidebarEl} transition:sidebarSlide|global>
      <div class="sidebar-header">
        <h2>
          <FolderGit2 size={16} />
          Repos
        </h2>
        <p>Manage per-repo actions and remove repos from Staged when they are no longer needed.</p>
      </div>
      <label class="sidebar-search">
        <Search size={14} />
        <Input
          bind:value={repoSearch}
          placeholder="Search"
          aria-label="Search repos"
          class="min-h-9 px-3 py-2"
        />
      </label>
      {#if loadingContexts}
        <div class="loading-side"><Spinner size={14} /> Loading...</div>
      {:else if mergedEntries.length === 0}
        <div class="empty-side">No repo contexts yet</div>
      {:else if filteredEntries.length === 0}
        <div class="empty-side">No repos match "{repoSearch.trim()}"</div>
      {:else}
        <div class="context-list">
          {#each filteredEntries as entry (entry.key)}
            {@const badge = repoBadgeStore.lookup(entry.githubRepo, entry.subpath)}
            <button
              class="context-item"
              class:selected={entry.key === selectedRepoKey}
              onclick={() => (selectedRepoKey = entry.key)}
            >
              <div class="context-item-main">
                <div class="context-item-header">
                  <RepoLabel githubRepo={entry.githubRepo} subpath={entry.subpath} />
                </div>
                <span class="context-meta">
                  {#if loadingRepoAttachments}
                    Loading usage...
                  {:else if badge && entry.context}
                    {@const count = (repoAttachmentsByContext[entry.context.id] ?? []).length}
                    <RepoBadge shortName={formatProjectCount(count)} hue={badge.hue} small />
                  {:else if entry.context}
                    {formatProjectCount((repoAttachmentsByContext[entry.context.id] ?? []).length)}
                  {:else if badge}
                    <RepoBadge shortName="orphan" hue={badge.hue} small />
                  {/if}
                </span>
              </div>
            </button>
          {/each}
        </div>
      {/if}
    </aside>

    <section class="main-panel" out:hideOnExit|global>
      <div class="main-panel-scroll">
        {#if !selectedEntry}
          <div class="empty-main">Select a repo to configure actions</div>
        {:else}
          <div class="repo-overview">
            <div class="repo-overview-main">
              <RepoLabel githubRepo={selectedEntry.githubRepo} subpath={selectedEntry.subpath} />
              <span class="repo-overview-meta">
                {#if loadingRepoAttachments}
                  Loading usage...
                {:else if selectedContext}
                  {formatProjectCount(selectedContextAttachments.length)}
                {:else}
                  Badge only (no action context)
                {/if}
              </span>
            </div>

            {#if selectedBadge}
              <div class="badge-editor">
                <div class="badge-editor-row">
                  <label class="badge-field">
                    <span class="badge-field-label">Short name</span>
                    <input
                      class="badge-input"
                      class:badge-input-error={badgeError}
                      type="text"
                      maxlength="6"
                      autocapitalize="off"
                      autocorrect="off"
                      bind:value={badgeEditName}
                      oninput={saveBadge}
                      onblur={saveBadge}
                      onkeydown={(e) => {
                        if (e.key === 'Enter') saveBadge();
                      }}
                    />
                  </label>
                  <label class="badge-field">
                    <span class="badge-field-label">Hue</span>
                    <input
                      class="badge-hue-slider"
                      type="range"
                      min="0"
                      max="359"
                      step="1"
                      style="background: {hueSliderGradient(darkMode.value)}"
                      bind:value={badgeEditHue}
                      onchange={saveBadge}
                    />
                  </label>
                  <div class="badge-editor-preview">
                    <RepoBadge
                      shortName={badgeEditName || selectedBadge.shortName}
                      hue={badgeEditHue}
                    />
                  </div>
                </div>
                {#if badgeError}
                  <span class="badge-error">{badgeError}</span>
                {/if}
              </div>
            {/if}

            {#if selectedContext}
              {#if selectedContextAttachments.length > 0}
                <div class="repo-attachments">
                  {#each selectedContextAttachments as attachment (attachment.projectRepoId)}
                    <span class="attachment-chip"
                      >{attachment.projectName} ({attachment.branchName})</span
                    >
                  {/each}
                </div>
              {:else}
                <div class="repo-empty-attachments">This repo is not attached to any projects.</div>
              {/if}
            {/if}
          </div>

          <div class="actions-header">
            <Button
              variant="destructive"
              size="sm"
              onclick={() => (showDeleteRepoConfirm = true)}
              disabled={deletingRepo}
            >
              {#if deletingRepo}
                <Spinner size={14} />
              {:else}
                <Trash2 size={14} />
              {/if}
              Delete Repo
            </Button>
            {#if selectedContext}
              {#if actions.length > 0}
                <Button
                  variant="outline"
                  size="sm"
                  onclick={() => (showDeleteAllConfirm = true)}
                  disabled={deletingRepo}
                >
                  <Trash2 size={14} />
                  Delete All Actions
                </Button>
              {/if}
              <Button
                variant="outline"
                size="sm"
                onclick={detectActions}
                disabled={detecting || deletingRepo}
              >
                {#if detecting}
                  <Spinner size={14} />
                {:else}
                  <Zap size={14} />
                {/if}
                Detect Actions
              </Button>
              <Button variant="outline" size="sm" onclick={startAddAction} disabled={deletingRepo}>
                <Plus size={14} />
                Add Action
              </Button>
            {/if}
          </div>

          {#if !selectedContext}
            <!-- Badge-only entry: no actions to show -->
          {:else if loadingActions}
            <div class="loading-state">
              <Spinner size={24} />
              <span>Loading...</span>
            </div>
          {:else if actions.length === 0}
            <div class="empty-state">
              <Play size={32} />
              <p>No actions configured</p>
              <p class="empty-hint">Click "Detect Actions" or add one manually</p>
            </div>
          {:else}
            <div class="actions-list">
              {#each Object.entries(groupedActions) as [type, typeActions]}
                {#if typeActions.length > 0}
                  <div class="action-group">
                    <div class="group-header">{type}</div>
                    {#each typeActions as action (action.id)}
                      <div class="action-row">
                        <div class="action-main">
                          <ActionIcon icon={action.icon} actionType={action.actionType} />
                          <div class="action-details">
                            <div class="action-name">
                              {action.name}
                              {#if action.pinned}
                                <span class="action-pinned" title="Shown in the card header">
                                  <Pin size={11} />
                                </span>
                              {/if}
                            </div>
                            <div class="action-command">
                              <Code2 size={12} />
                              {action.command}
                            </div>
                          </div>
                        </div>
                        <div class="action-buttons">
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            onclick={() => startEditAction(action)}
                          >
                            <Pencil size={13} />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            class="hover:text-destructive"
                            onclick={() => deleteAction(action.id)}
                          >
                            <Trash2 size={13} />
                          </Button>
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}
              {/each}
            </div>
          {/if}
        {/if}
      </div>

      {#if editingAction}
        <div class="editor">
          <IconPicker
            icon={editForm.icon}
            actionType={editForm.actionType}
            onSelect={(icon) => (editForm.icon = icon)}
          />
          <Input bind:value={editForm.name} placeholder="Action name" />
          <Input
            bind:value={editForm.command}
            placeholder="Command"
            autocomplete="off"
            autocorrect="off"
            autocapitalize="off"
            spellcheck="false"
          />
          <Select.Root
            type="single"
            value={editForm.actionType}
            onValueChange={(v) => (editForm.actionType = v as ActionType)}
          >
            <Select.Trigger class="w-full">
              {editForm.actionType}
            </Select.Trigger>
            <Select.Content>
              {#each ['run', 'prerun', 'build', 'test', 'format', 'check', 'cleanUp'] as t (t)}
                <Select.Item value={t} label={t}>{t}</Select.Item>
              {/each}
            </Select.Content>
          </Select.Root>
          <div class="flex items-center gap-1.5">
            <Checkbox id="auto-commit" bind:checked={editForm.autoCommit} />
            <Label for="auto-commit" class="text-muted-foreground text-sm">Auto-commit</Label>
          </div>
          <div class="flex items-center gap-1.5">
            <Checkbox id="pinned" bind:checked={editForm.pinned} />
            <Label for="pinned" class="text-muted-foreground text-sm">Show in card header</Label>
          </div>
          <div class="editor-buttons">
            <Button variant="ghost" size="sm" onclick={cancelEdit}>Cancel</Button>
            <Button variant="outline" size="sm" onclick={saveAction}>
              <Save size={14} />
              Save
            </Button>
          </div>
        </div>
      {/if}
    </section>
  </div>
</div>

<AlertDialog.Root bind:open={showDeleteRepoConfirm}>
  <AlertDialog.Content>
    {#if selectedEntry}
      <AlertDialog.Header>
        <AlertDialog.Title>Delete Repo</AlertDialog.Title>
        <AlertDialog.Description>
          {selectedContextAttachments.length > 0
            ? `Delete "${repoDisplay(selectedEntry.githubRepo, selectedEntry.subpath)}" from Staged? This removes ${formatProjectCount(selectedContextAttachments.length)} and deletes tracked worktrees/workspaces tied to this repo.`
            : `Delete "${repoDisplay(selectedEntry.githubRepo, selectedEntry.subpath)}" from Staged? This removes its repo settings and actions.`}
        </AlertDialog.Description>
      </AlertDialog.Header>
      <AlertDialog.Footer>
        <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
        <AlertDialog.Action variant="destructive" onclick={deleteRepo}>
          Delete Repo
        </AlertDialog.Action>
      </AlertDialog.Footer>
    {/if}
  </AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={showDeleteAllConfirm}>
  <AlertDialog.Content>
    <AlertDialog.Header>
      <AlertDialog.Title>Delete All Actions</AlertDialog.Title>
      <AlertDialog.Description>
        Are you sure you want to delete all actions for this repo? This action cannot be undone.
      </AlertDialog.Description>
    </AlertDialog.Header>
    <AlertDialog.Footer>
      <AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
      <AlertDialog.Action variant="destructive" onclick={deleteAllActions}>
        Delete All
      </AlertDialog.Action>
    </AlertDialog.Footer>
  </AlertDialog.Content>
</AlertDialog.Root>

<style>
  .actions-settings-panel {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: transparent;
  }

  .panel-body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .sidebar {
    --repo-row-bleed: 10px;
    --repo-sidebar-hover-bg: color-mix(in srgb, var(--text-primary) 4%, transparent);

    position: relative;
    width: 260px;
    flex-shrink: 0;
    border-right: 1px solid color-mix(in srgb, var(--border-subtle) 50%, transparent);
    background: var(--bg-app-bar);
    padding: 0 var(--repo-row-bleed) 10px;
    overflow-y: auto;
    min-height: 0;
  }

  .sidebar-header {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px 2px 12px;
  }

  .sidebar-header h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-md);
    font-weight: 600;
    color: var(--text-primary);
  }

  .sidebar-header p {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.35;
  }

  .sidebar-search {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
    padding: 0 2px;
    color: var(--text-faint);
  }

  .context-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .context-item {
    width: calc(100% + (2 * var(--repo-row-bleed)));
    margin: 0 calc(-1 * var(--repo-row-bleed));
    text-align: left;
    padding: 8px 10px;
    border: none;
    border-radius: 0;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font-size: var(--size-sm);
    transition:
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .context-item:hover {
    background: var(--repo-sidebar-hover-bg);
  }

  .context-item.selected {
    background: var(--bg-hover);
  }

  .context-item-main {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }

  .context-item-header {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .context-meta {
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-faint);
  }

  .loading-side,
  .empty-side {
    color: var(--text-muted);
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 18px 10px;
    font-size: var(--size-sm);
  }

  .empty-main,
  .loading-state,
  .empty-state {
    color: var(--text-muted);
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
  }

  .main-panel {
    flex: 1;
    min-width: 0;
    background: var(--bg-chrome);
    padding: 14px;
    overflow: hidden;
    min-height: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .main-panel-scroll {
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 12px;
  }

  .empty-main,
  .loading-state,
  .empty-state {
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-primary);
  }

  .empty-main,
  .loading-state {
    min-height: 180px;
    padding: 24px;
  }

  .repo-overview {
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-primary);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .repo-overview-main {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .repo-overview-meta {
    font-size: var(--size-xs);
    color: var(--text-muted);
    white-space: nowrap;
  }

  .badge-editor {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 10px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-chrome);
  }

  .badge-editor-row {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }

  .badge-editor-preview {
    flex-shrink: 0;
  }

  .badge-field {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .badge-field-label {
    white-space: nowrap;
  }

  .badge-input {
    width: 60px;
    padding: 3px 6px;
    border-radius: 6px;
    border: 1px solid var(--border-muted);
    background: var(--bg-primary);
    color: var(--text-primary);
    font-family: 'SF Mono', Menlo, Consolas, monospace;
    font-size: var(--size-md);
  }

  .badge-input-error {
    border-color: var(--ui-danger);
  }

  .badge-error {
    color: var(--ui-danger);
    font-size: var(--size-xs);
  }

  .badge-hue-slider {
    width: 200px;
    cursor: pointer;
    -webkit-appearance: none;
    appearance: none;
    height: 16px;
    border-radius: 8px;
    /* Track gradient comes from hueSliderGradient() as an inline style so it
       tracks the theme and stays in sync with the badge OKLCH values. */
    border: 1px solid var(--border-subtle);
    outline: none;
  }

  .badge-hue-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--bg-primary);
    border: 2px solid var(--border-emphasis);
    box-shadow: 0 1px 3px color-mix(in srgb, var(--text-primary) 20%, transparent);
    cursor: pointer;
  }

  .badge-hue-slider::-moz-range-thumb {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: var(--bg-primary);
    border: 2px solid var(--border-emphasis);
    box-shadow: 0 1px 3px color-mix(in srgb, var(--text-primary) 20%, transparent);
    cursor: pointer;
  }

  .repo-attachments {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .attachment-chip {
    border: 1px solid var(--border-subtle);
    border-radius: 999px;
    padding: 4px 8px;
    font-size: calc(var(--size-xs) - 1px);
    color: var(--text-muted);
    background: var(--bg-chrome);
  }

  .repo-empty-attachments {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .actions-header {
    display: flex;
    justify-content: flex-start;
    flex-wrap: wrap;
    gap: 8px;
  }

  .actions-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .action-group {
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-primary);
  }

  .group-header {
    background: var(--bg-chrome);
    color: var(--text-muted);
    font-size: var(--size-xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    padding: 8px 10px;
  }

  .action-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 10px;
    border-top: 1px solid var(--border-subtle);
  }

  .action-main {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }

  .action-name {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: var(--size-sm);
    font-weight: 600;
  }

  /* Marks the actions that occupy a slot in every card's header. */
  .action-pinned {
    display: inline-flex;
    color: var(--text-faint);
  }

  .action-command {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-muted);
    font-family: 'SF Mono', Menlo, Monaco, monospace;
    font-size: var(--size-xs);
    margin-top: 3px;
  }

  .action-buttons {
    display: flex;
    gap: 4px;
  }

  .editor {
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background: var(--bg-primary);
    padding: 12px;
    display: grid;
    grid-template-columns: auto 1fr 1fr auto auto auto;
    gap: 8px;
    align-items: center;
  }

  .editor-buttons {
    display: inline-flex;
    gap: 8px;
    grid-column: 1 / -1;
    justify-self: end;
  }

  @media (max-width: 900px) {
    .panel-body {
      flex-direction: column;
    }

    .sidebar {
      width: auto;
      border-right: none;
      border-bottom: 1px solid var(--border-subtle);
      max-height: 160px;
    }

    .repo-overview-main {
      align-items: flex-start;
      flex-direction: column;
    }

    .actions-header :global(button) {
      flex: 1 1 auto;
      justify-content: center;
    }

    .editor {
      grid-template-columns: 1fr;
    }

    .editor-buttons {
      justify-self: stretch;
    }
  }
</style>
