<script lang="ts">
  import { onMount, onDestroy, untrack } from 'svelte';
  import {
    FolderGit2,
    Play,
    Hammer,
    FlaskConical,
    Wand2,
    CheckCircle,
    Zap,
    Plus,
    Trash2,
    Save,
    Pencil,
    Code2,
    Search,
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import FormInput from '../../shared/FormInput.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import RepoBadge from '../../shared/RepoBadge.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import type { ActionContext, ProjectAction } from '../../api/commands';
  import * as commands from '../../api/commands';
  import {
    detectRepoActions,
    listenToRepoActionsDetection,
    type ActionType,
  } from '../actions/actions';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { matchesRepoSearch } from './repoContextSearch';
  import { alerts } from '../../shared/alerts.svelte';
  import { getPreferredAgent } from './preferences.svelte';
  import { agentState } from '../agents/agent.svelte';

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
  });
  let badgeEditName = $state('');
  let badgeEditHue = $state(0);
  let badgeError = $state('');

  let unlistenDetection: (() => void) | undefined;

  onMount(async () => {
    listenToRepoActionsDetection((event) => {
      const ctx = selectedContext;
      if (!ctx) return;
      if (event.githubRepo !== ctx.githubRepo || event.subpath !== (ctx.subpath ?? null)) return;
      detecting = event.detecting;
      if (!event.detecting) {
        loadActions();
      }
    }).then((unlisten) => {
      unlistenDetection = unlisten;
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
      const projects = await commands.listProjects();
      const reposByProject = await Promise.all(
        projects.map(async (project) => {
          const repos = await commands.listProjectRepos(project.id);
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
    loadingContexts = true;
    try {
      const nextContexts = await commands.listActionContexts();
      contexts = nextContexts;
      await loadRepoAttachments(nextContexts);
      if (!selectedRepoKey && mergedEntries.length > 0) {
        selectedRepoKey = mergedEntries[0].key;
      } else if (selectedRepoKey && !mergedEntries.some((e) => e.key === selectedRepoKey)) {
        selectedRepoKey = mergedEntries.length > 0 ? mergedEntries[0].key : null;
      }
      await loadActions();
    } catch (e) {
      console.error('Failed to load action contexts:', e);
    } finally {
      loadingContexts = false;
    }
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
      const suggested = await detectRepoActions(githubRepo, subpath, provider);

      // After the await the user may have navigated away from this context.
      // Only mutate local `actions` state when we're still viewing the same
      // context; the backend writes are always scoped to the captured values.
      const existingCommands = new Set(actions.map((a) => a.command));
      let nextSortOrder = Math.max(...actions.map((a) => a.sortOrder), 0) + 1;

      let actionsAdded = false;
      for (const suggestion of suggested) {
        if (existingCommands.has(suggestion.command)) continue;
        const newAction = await commands.createRepoAction(
          githubRepo,
          subpath,
          suggestion.name,
          suggestion.command,
          suggestion.actionType,
          nextSortOrder++,
          suggestion.autoCommit
        );
        if (selectedRepoKey === entryKey) {
          actions = [...actions, newAction];
        }
        actionsAdded = true;
      }

      if (actionsAdded) {
        window.dispatchEvent(new CustomEvent('project-actions-changed'));
      }

      await loadContexts();
    } catch (e) {
      console.error('Failed to detect actions:', e);
      alerts.error(String(e), 'Failed to detect actions');
    } finally {
      detecting = false;
    }
  }

  function startAddAction() {
    editForm = { name: '', command: '', actionType: 'run', autoCommit: false };
    editingAction = {} as ProjectAction;
  }

  function startEditAction(action: ProjectAction) {
    editForm = {
      name: action.name,
      command: action.command,
      actionType: action.actionType as ActionType,
      autoCommit: action.autoCommit,
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
          editForm.autoCommit
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
          editForm.autoCommit
        );
        actions = actions.map((a) =>
          a.id === actionId
            ? {
                ...a,
                name: editForm.name,
                command: editForm.command,
                actionType: editForm.actionType,
                autoCommit: editForm.autoCommit,
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

  function getActionIcon(actionType: string) {
    switch (actionType) {
      case 'prerun':
        return Zap;
      case 'build':
        return Hammer;
      case 'test':
        return FlaskConical;
      case 'format':
        return Wand2;
      case 'check':
        return CheckCircle;
      case 'run':
        return Play;
      case 'cleanUp':
        return Trash2;
      default:
        return Play;
    }
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
  <div class="panel-intro">
    <h2>
      <FolderGit2 size={16} />
      Repos
    </h2>
    <p>Manage per-repo actions and remove repos from Staged when they are no longer needed.</p>
  </div>

  <div class="panel-body">
    <aside class="sidebar">
      <div class="sidebar-title">Repos</div>
      <label class="sidebar-search">
        <Search size={14} />
        <FormInput bind:value={repoSearch} placeholder="Search" aria-label="Search repos" />
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

    <section class="main-panel">
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
          <button
            class="danger-btn"
            onclick={() => (showDeleteRepoConfirm = true)}
            disabled={deletingRepo}
          >
            {#if deletingRepo}
              <Spinner size={14} />
            {:else}
              <Trash2 size={14} />
            {/if}
            Delete Repo
          </button>
          {#if selectedContext}
            {#if actions.length > 0}
              <button
                class="secondary-btn"
                onclick={() => (showDeleteAllConfirm = true)}
                disabled={deletingRepo}
              >
                <Trash2 size={14} />
                Delete All Actions
              </button>
            {/if}
            <button
              class="secondary-btn"
              onclick={detectActions}
              disabled={detecting || deletingRepo}
            >
              {#if detecting}
                <Spinner size={14} />
              {:else}
                <Zap size={14} />
              {/if}
              Detect Actions
            </button>
            <button class="primary-btn" onclick={startAddAction} disabled={deletingRepo}>
              <Plus size={14} />
              Add Action
            </button>
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
                    {@const Icon = getActionIcon(action.actionType)}
                    <div class="action-row">
                      <div class="action-main">
                        <Icon size={14} />
                        <div class="action-details">
                          <div class="action-name">{action.name}</div>
                          <div class="action-command">
                            <Code2 size={12} />
                            {action.command}
                          </div>
                        </div>
                      </div>
                      <div class="action-buttons">
                        <button class="icon-btn" onclick={() => startEditAction(action)}>
                          <Pencil size={13} />
                        </button>
                        <button class="icon-btn danger" onclick={() => deleteAction(action.id)}>
                          <Trash2 size={13} />
                        </button>
                      </div>
                    </div>
                  {/each}
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      {/if}
    </section>
  </div>

  {#if editingAction}
    <div class="editor">
      <input bind:value={editForm.name} placeholder="Action name" />
      <input
        bind:value={editForm.command}
        placeholder="Command"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck="false"
      />
      <select bind:value={editForm.actionType}>
        <option value="run">run</option>
        <option value="prerun">prerun</option>
        <option value="build">build</option>
        <option value="test">test</option>
        <option value="format">format</option>
        <option value="check">check</option>
        <option value="cleanUp">cleanUp</option>
      </select>
      <label class="checkbox-row">
        <input type="checkbox" bind:checked={editForm.autoCommit} />
        Auto-commit
      </label>
      <div class="editor-buttons">
        <button class="secondary-btn" onclick={cancelEdit}>Cancel</button>
        <button class="primary-btn" onclick={saveAction}>
          <Save size={14} />
          Save
        </button>
      </div>
    </div>
  {/if}
</div>

{#if showDeleteRepoConfirm && selectedEntry}
  <ConfirmDialog
    title="Delete Repo"
    message={selectedContextAttachments.length > 0
      ? `Delete "${repoDisplay(selectedEntry.githubRepo, selectedEntry.subpath)}" from Staged? This removes ${formatProjectCount(selectedContextAttachments.length)} and deletes tracked worktrees/workspaces tied to this repo.`
      : `Delete "${repoDisplay(selectedEntry.githubRepo, selectedEntry.subpath)}" from Staged? This removes its repo settings and actions.`}
    confirmLabel="Delete Repo"
    danger={true}
    onConfirm={deleteRepo}
    onCancel={() => (showDeleteRepoConfirm = false)}
  />
{/if}

{#if showDeleteAllConfirm}
  <ConfirmDialog
    title="Delete All Actions"
    message="Are you sure you want to delete all actions for this repo? This action cannot be undone."
    confirmLabel="Delete All"
    danger={true}
    onConfirm={deleteAllActions}
    onCancel={() => (showDeleteAllConfirm = false)}
  />
{/if}

<style>
  .actions-settings-panel {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-subtle);
    border-radius: 12px;
    overflow: hidden;
    background: var(--bg-chrome);
  }

  .panel-intro {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .panel-intro h2 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--size-md);
    font-weight: 600;
  }

  .panel-intro p {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .icon-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  .icon-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .icon-btn.danger:hover {
    color: var(--ui-danger);
  }

  .panel-body {
    display: grid;
    grid-template-columns: 260px 1fr;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .sidebar {
    border-right: 1px solid var(--border-subtle);
    padding: 10px;
    overflow-y: auto;
    min-height: 0;
  }

  .sidebar-title {
    font-size: var(--size-xs);
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    margin: 4px 6px 10px;
  }

  .sidebar-search {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 8px;
    margin-bottom: 10px;
    padding: 0 2px;
    color: var(--text-faint);
  }

  .sidebar-search :global(.form-input) {
    min-width: 0;
    min-height: 36px;
    padding: 8px 12px;
    font-size: var(--size-md);
  }

  .context-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .context-item {
    text-align: left;
    padding: 8px 10px;
    border: 1px solid transparent;
    border-radius: 8px;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
    font-size: var(--size-sm);
  }

  .context-item:hover {
    background: var(--bg-hover);
  }

  .context-item.selected {
    background: var(--bg-primary);
    border-color: var(--border-muted);
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
  .empty-side,
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
    padding: 14px;
    overflow-y: auto;
    min-height: 0;
  }

  .repo-overview {
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    background: color-mix(in srgb, var(--bg-primary) 70%, transparent);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-bottom: 12px;
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
    background: var(--bg-primary);
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
    background: var(--bg-chrome);
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
    background: linear-gradient(
      to right,
      hsl(0, 80%, 55%),
      hsl(30, 80%, 55%),
      hsl(60, 80%, 55%),
      hsl(90, 80%, 55%),
      hsl(120, 80%, 55%),
      hsl(150, 80%, 55%),
      hsl(180, 80%, 55%),
      hsl(210, 80%, 55%),
      hsl(240, 80%, 55%),
      hsl(270, 80%, 55%),
      hsl(300, 80%, 55%),
      hsl(330, 80%, 55%),
      hsl(359, 80%, 55%)
    );
    outline: none;
  }

  .badge-hue-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: white;
    border: 2px solid rgba(0, 0, 0, 0.3);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
    cursor: pointer;
  }

  .badge-hue-slider::-moz-range-thumb {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: white;
    border: 2px solid rgba(0, 0, 0, 0.3);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.2);
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
    background: var(--bg-primary);
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
    margin-bottom: 12px;
  }

  .primary-btn,
  .secondary-btn,
  .danger-btn {
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    padding: 7px 10px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    cursor: pointer;
  }

  .primary-btn {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
    color: white;
  }

  .secondary-btn {
    background: var(--bg-primary);
    color: var(--text-primary);
  }

  .danger-btn {
    background: var(--bg-primary);
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .danger-btn:hover:not(:disabled) {
    background: var(--ui-danger);
    color: white;
  }

  .primary-btn:disabled,
  .secondary-btn:disabled,
  .danger-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .actions-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .action-group {
    border: 1px solid var(--border-subtle);
    border-radius: 10px;
    overflow: hidden;
  }

  .group-header {
    background: var(--bg-primary);
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
    font-size: var(--size-sm);
    font-weight: 600;
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
    border-top: 1px solid var(--border-subtle);
    padding: 12px;
    display: grid;
    grid-template-columns: 1fr 1fr auto auto;
    gap: 8px;
    align-items: center;
  }

  .editor input,
  .editor select {
    padding: 8px 10px;
    border-radius: 8px;
    border: 1px solid var(--border-muted);
    background: var(--bg-primary);
    color: var(--text-primary);
    min-width: 0;
  }

  .checkbox-row {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .editor-buttons {
    display: inline-flex;
    gap: 8px;
    justify-self: end;
  }

  @media (max-width: 900px) {
    .panel-body {
      grid-template-columns: 1fr;
    }

    .sidebar {
      border-right: none;
      border-bottom: 1px solid var(--border-subtle);
      max-height: 160px;
    }

    .repo-overview-main {
      align-items: flex-start;
      flex-direction: column;
    }

    .actions-header button {
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
