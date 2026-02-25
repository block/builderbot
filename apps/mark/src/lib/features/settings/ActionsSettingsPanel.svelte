<script lang="ts">
  import { onMount } from 'svelte';
  import {
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
  } from 'lucide-svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import ConfirmDialog from '../../shared/ConfirmDialog.svelte';
  import type { ActionContext, ProjectAction } from '../../commands';
  import * as commands from '../../commands';
  import { detectRepoActions, type ActionType } from '../actions/actions';

  let contexts = $state<ActionContext[]>([]);
  let selectedContextId = $state<string | null>(null);
  let loadingContexts = $state(false);

  let actions = $state<ProjectAction[]>([]);
  let loadingActions = $state(false);
  let detecting = $state(false);
  let showDeleteAllConfirm = $state(false);
  let editingAction = $state<ProjectAction | null>(null);
  let editForm = $state({
    name: '',
    command: '',
    actionType: 'run' as ActionType,
    autoCommit: false,
  });

  let selectedContext = $derived(contexts.find((c) => c.id === selectedContextId) ?? null);

  onMount(async () => {
    await loadContexts();
  });

  async function loadContexts() {
    loadingContexts = true;
    try {
      contexts = await commands.listActionContexts();
      if (!selectedContextId && contexts.length > 0) {
        selectedContextId = contexts[0].id;
      } else if (selectedContextId && !contexts.some((c) => c.id === selectedContextId)) {
        selectedContextId = contexts.length > 0 ? contexts[0].id : null;
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
    selectedContextId;
    loadActions();
  });

  async function detectActions() {
    if (!selectedContext) return;

    detecting = true;
    try {
      const suggested = await detectRepoActions(
        selectedContext.githubRepo,
        selectedContext.subpath ?? undefined
      );

      const existingCommands = new Set(actions.map((a) => a.command));
      let nextSortOrder = Math.max(...actions.map((a) => a.sortOrder), 0) + 1;

      let actionsAdded = false;
      for (const suggestion of suggested) {
        if (existingCommands.has(suggestion.command)) continue;
        const newAction = await commands.createRepoAction(
          selectedContext.githubRepo,
          selectedContext.subpath ?? undefined,
          suggestion.name,
          suggestion.command,
          suggestion.actionType,
          nextSortOrder++,
          suggestion.autoCommit
        );
        actions = [...actions, newAction];
        actionsAdded = true;
      }

      if (actionsAdded) {
        window.dispatchEvent(new CustomEvent('project-actions-changed'));
      }

      contexts = await commands.listActionContexts();
    } catch (e) {
      console.error('Failed to detect actions:', e);
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

    try {
      if (!editingAction?.id) {
        const nextSortOrder = Math.max(...actions.map((a) => a.sortOrder), 0) + 1;
        const newAction = await commands.createRepoAction(
          selectedContext.githubRepo,
          selectedContext.subpath ?? undefined,
          editForm.name,
          editForm.command,
          editForm.actionType,
          nextSortOrder,
          editForm.autoCommit
        );
        actions = [...actions, newAction];
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
    try {
      await commands.deleteAllRepoActions(selectedContext.id);
      actions = [];
      showDeleteAllConfirm = false;
      window.dispatchEvent(new CustomEvent('project-actions-changed'));
    } catch (e) {
      console.error('Failed to delete all actions:', e);
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

  let sortedContexts = $derived.by(() => {
    return [...contexts].sort((a, b) => {
      const aDisplay = a.subpath ? `${a.githubRepo}/${a.subpath}` : a.githubRepo;
      const bDisplay = b.subpath ? `${b.githubRepo}/${b.subpath}` : b.githubRepo;
      return aDisplay.localeCompare(bDisplay);
    });
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
      <Play size={16} />
      Actions
    </h2>
    <p>Configure per-repository action commands used across your projects.</p>
  </div>

  <div class="panel-body">
    <aside class="sidebar">
      <div class="sidebar-title">Repos</div>
      {#if loadingContexts}
        <div class="loading-side"><Spinner size={14} /> Loading...</div>
      {:else if contexts.length === 0}
        <div class="empty-side">No repo contexts yet</div>
      {:else}
        <div class="context-list">
          {#each sortedContexts as context (context.id)}
            <button
              class="context-item"
              class:selected={context.id === selectedContextId}
              onclick={() => (selectedContextId = context.id)}
            >
              <RepoLabel githubRepo={context.githubRepo} subpath={context.subpath} />
            </button>
          {/each}
        </div>
      {/if}
    </aside>

    <section class="main-panel">
      {#if !selectedContext}
        <div class="empty-main">Select a repo context to configure actions</div>
      {:else}
        <div class="actions-header">
          {#if actions.length > 0}
            <button class="danger-btn" onclick={() => (showDeleteAllConfirm = true)}>
              <Trash2 size={14} />
              Delete All
            </button>
          {/if}
          <button class="secondary-btn" onclick={detectActions} disabled={detecting}>
            {#if detecting}
              <Spinner size={14} />
            {:else}
              <Zap size={14} />
            {/if}
            Detect Actions
          </button>
          <button class="primary-btn" onclick={startAddAction}>
            <Plus size={14} />
            Add Action
          </button>
        </div>

        {#if loadingActions}
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
      <input bind:value={editForm.command} placeholder="Command" />
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

  .actions-header {
    display: flex;
    justify-content: flex-end;
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

  .danger-btn:hover {
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

    .editor {
      grid-template-columns: 1fr;
    }

    .editor-buttons {
      justify-self: stretch;
    }
  }
</style>
