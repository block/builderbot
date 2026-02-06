<!--
  ProjectSettingsModal.svelte - Manage project actions
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    X,
    Settings,
    Play,
    Wand,
    CheckCircle,
    Zap,
    Plus,
    Trash2,
    Loader2,
    Save,
    Pencil,
    FlaskConical,
    BrushCleaning,
    Hammer,
  } from 'lucide-svelte';
  import type { GitProject, ProjectAction, ActionType, SuggestedAction } from './services/branch';
  import * as branchService from './services/branch';

  interface Props {
    project: GitProject;
    onClose: () => void;
    onUpdated?: (project: GitProject) => void;
  }

  let { project, onClose, onUpdated }: Props = $props();

  // Actions state
  let actions = $state<ProjectAction[]>([]);
  let loadingActions = $state(false);
  let detecting = $state(false);
  let editingAction = $state<ProjectAction | null>(null);
  let editForm = $state({
    name: '',
    command: '',
    actionType: 'run' as ActionType,
    autoCommit: false,
  });

  // Load actions on mount
  onMount(() => {
    loadActions();
  });

  async function loadActions() {
    loadingActions = true;
    try {
      actions = await branchService.listProjectActions(project.id);
    } catch (e) {
      console.error('Failed to load actions:', e);
    } finally {
      loadingActions = false;
    }
  }

  async function detectActions() {
    detecting = true;
    try {
      const suggested = await branchService.detectProjectActions(project.id);

      // Add suggested actions that don't already exist
      const existingCommands = new Set(actions.map((a) => a.command));
      let nextSortOrder = Math.max(...actions.map((a) => a.sortOrder), 0) + 1;

      for (const suggestion of suggested) {
        if (!existingCommands.has(suggestion.command)) {
          const newAction = await branchService.createProjectAction(
            project.id,
            suggestion.name,
            suggestion.command,
            suggestion.actionType,
            nextSortOrder++,
            suggestion.autoCommit
          );
          actions = [...actions, newAction];
        }
      }
    } catch (e) {
      console.error('Failed to detect actions:', e);
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
    };
    editingAction = {} as ProjectAction; // Empty object signals "adding"
  }

  function startEditAction(action: ProjectAction) {
    editForm = {
      name: action.name,
      command: action.command,
      actionType: action.actionType,
      autoCommit: action.autoCommit,
    };
    editingAction = action;
  }

  function cancelEdit() {
    editingAction = null;
  }

  async function saveAction() {
    if (!editForm.name || !editForm.command) return;

    try {
      if (!editingAction?.id) {
        // Adding new action
        const nextSortOrder = Math.max(...actions.map((a) => a.sortOrder), 0) + 1;
        const newAction = await branchService.createProjectAction(
          project.id,
          editForm.name,
          editForm.command,
          editForm.actionType,
          nextSortOrder,
          editForm.autoCommit
        );
        actions = [...actions, newAction];
      } else if (editingAction) {
        // Updating existing action
        const actionId = editingAction.id;
        await branchService.updateProjectAction(
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
    } catch (e) {
      console.error('Failed to save action:', e);
    }
  }

  async function deleteAction(actionId: string) {
    try {
      await branchService.deleteProjectAction(actionId);
      actions = actions.filter((a) => a.id !== actionId);
    } catch (e) {
      console.error('Failed to delete action:', e);
    }
  }

  function getActionIcon(actionType: ActionType) {
    switch (actionType) {
      case 'prerun':
        return Zap;
      case 'run':
        return Play;
      case 'build':
        return Hammer;
      case 'format':
        return Wand;
      case 'check':
        return CheckCircle;
      case 'test':
        return FlaskConical;
      case 'cleanUp':
        return BrushCleaning;
    }
  }

  function getActionTypeColor(actionType: ActionType): string {
    return 'var(--ui-accent)';
  }

  // Group actions by type
  let groupedActions = $derived.by(() => {
    const groups: Record<ActionType, ProjectAction[]> = {
      prerun: [],
      run: [],
      build: [],
      format: [],
      check: [],
      test: [],
      cleanUp: [],
    };
    for (const action of actions) {
      groups[action.actionType].push(action);
    }
    return groups;
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && !editingAction) {
      onClose();
      event.preventDefault();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget && !editingAction) {
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && !editingAction && onClose()}
>
  <div class="modal">
    <header class="modal-header">
      <h2>
        <Play size={16} />
        Actions
      </h2>
      <button class="close-btn" onclick={onClose}>
        <X size={16} />
      </button>
    </header>

    <div class="modal-body">
      <div class="content-wrapper">
        <div class="actions-header">
          <button class="secondary-btn" onclick={detectActions} disabled={detecting}>
            {#if detecting}
              <Loader2 size={14} class="spinner" />
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
            <Loader2 size={24} />
            <span>Loading...</span>
          </div>
        {:else if actions.length === 0}
          <div class="empty-state">
            <Play size={32} />
            <p>No actions configured</p>
            <p class="empty-hint">
              Click "Detect Actions" to find common scripts, or add one manually
            </p>
          </div>
        {:else}
          <div class="actions-list">
            {#each Object.entries(groupedActions) as [type, typeActions]}
              {#if typeActions.length > 0}
                <div class="action-group">
                  <div class="group-header" style="color: {getActionTypeColor(type as ActionType)}">
                    <!-- svelte-ignore svelte_component_deprecated -->
                    <svelte:component this={getActionIcon(type as ActionType)} size={14} />
                    {type.charAt(0).toUpperCase() + type.slice(1)}
                  </div>
                  {#if type === 'prerun'}
                    <div class="group-subtitle">
                      These actions will run automatically when a new worktree is created
                    </div>
                  {/if}
                  {#each typeActions as action (action.id)}
                    <div class="action-item">
                      <div class="action-info">
                        <div class="action-name">
                          {action.name}
                          {#if action.autoCommit}
                            <span class="action-badge">Commits to git</span>
                          {/if}
                        </div>
                        <code class="action-command">{action.command}</code>
                      </div>
                      <div class="action-controls">
                        <button
                          class="icon-btn"
                          onclick={() => startEditAction(action)}
                          title="Edit"
                        >
                          <Pencil size={14} />
                        </button>
                        <button
                          class="icon-btn danger"
                          onclick={() => deleteAction(action.id)}
                          title="Delete"
                        >
                          <Trash2 size={14} />
                        </button>
                      </div>
                    </div>
                  {/each}
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      </div>
    </div>
  </div>

  <!-- Edit Action Modal (separate overlay) -->
  {#if editingAction}
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="edit-modal-backdrop" role="presentation" onclick={cancelEdit}>
      <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
      <div class="edit-modal" role="dialog" tabindex="-1" onclick={(e) => e.stopPropagation()}>
        <header class="edit-modal-header">
          <h3>{editingAction.id ? 'Edit Action' : 'New Action'}</h3>
          <button class="close-btn" onclick={cancelEdit}>
            <X size={16} />
          </button>
        </header>

        <div class="edit-modal-body">
          <div class="form-group">
            <label for="action-name">Name</label>
            <input
              id="action-name"
              type="text"
              bind:value={editForm.name}
              placeholder="e.g., Lint"
            />
          </div>

          <div class="form-group">
            <label for="action-command">Command</label>
            <input
              id="action-command"
              type="text"
              bind:value={editForm.command}
              placeholder="e.g., npm run lint"
            />
          </div>

          <div class="form-group">
            <label for="action-type">Type</label>
            <select id="action-type" bind:value={editForm.actionType}>
              <option value="run">Run - Manual execution</option>
              <option value="format">Format - Auto-fix issues</option>
              <option value="check">Check - Validation only</option>
              <option value="test">Test - Run tests</option>
              <option value="cleanUp">Clean Up - Remove build artifacts</option>
              <option value="prerun">Prerun - Auto-run on branch creation</option>
            </select>
            {#if editForm.actionType === 'prerun'}
              <div class="type-hint">
                Prerun actions will run automatically when a new worktree is created
              </div>
            {/if}
          </div>

          <div class="form-group checkbox-group">
            <label>
              <input type="checkbox" bind:checked={editForm.autoCommit} />
              Auto-commit changes after successful execution
            </label>
          </div>
        </div>

        <footer class="edit-modal-footer">
          <button class="secondary-btn" onclick={cancelEdit}> Cancel </button>
          <button
            class="primary-btn"
            onclick={saveAction}
            disabled={!editForm.name || !editForm.command}
          >
            <Save size={14} />
            Save
          </button>
        </footer>
      </div>
    </div>
  {/if}
</div>

<style>
  .modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    background: var(--bg-chrome);
    border-radius: 12px;
    width: min(700px, 90vw);
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--shadow-elevated);
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .close-btn {
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 4px;
    display: flex;
    align-items: center;
    border-radius: 4px;
    transition: all 0.15s;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-body {
    flex: 1;
    overflow-y: auto;
    padding: 20px;
  }

  .content-wrapper {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .primary-btn,
  .secondary-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    border-radius: 6px;
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s;
  }

  .primary-btn {
    background: var(--ui-accent);
    color: var(--bg-primary);
    border: none;
  }

  .primary-btn:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .primary-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .secondary-btn {
    background: var(--bg-primary);
    color: var(--text-primary);
    border: 1px solid var(--border-muted);
  }

  .secondary-btn:hover:not(:disabled) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  .secondary-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .actions-header {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .loading-state,
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 60px 20px;
    color: var(--text-secondary);
    text-align: center;
    gap: 12px;
  }

  .empty-state {
    gap: 12px;
  }

  .empty-state p {
    margin: 0;
  }

  .empty-hint {
    font-size: 12px;
    color: var(--text-tertiary);
  }

  .actions-list {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .action-group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .group-header {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    margin-bottom: 4px;
  }

  .group-subtitle {
    font-size: 12px;
    color: var(--text-secondary);
    margin-bottom: 8px;
    font-weight: 400;
  }

  .action-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
  }

  .action-info {
    flex: 1;
    min-width: 0;
  }

  .action-name {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 4px;
  }

  .action-command {
    display: inline-block;
    font-size: 12px;
    color: var(--text-secondary);
    font-family: var(--font-mono);
    background: var(--bg-hover);
    padding: 4px 8px;
    border-radius: 4px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-top: 4px;
    max-width: 100%;
  }

  .action-badge {
    display: inline-flex;
    align-items: center;
    font-size: 10px;
    padding: 3px 8px;
    background: var(--bg-hover);
    color: var(--text-secondary);
    border-radius: 4px;
    font-weight: 400;
    white-space: nowrap;
  }

  .action-controls {
    display: flex;
    gap: 4px;
  }

  .icon-btn {
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    padding: 6px;
    display: flex;
    align-items: center;
    border-radius: 4px;
    transition: all 0.15s;
  }

  .icon-btn:hover {
    background: var(--bg-primary);
    color: var(--text-primary);
  }

  .icon-btn.danger:hover {
    background: var(--color-error);
    color: white;
  }

  /* Edit Modal */
  .edit-modal-backdrop {
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: transparent;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1100;
  }

  .edit-modal {
    background: var(--bg-chrome);
    border-radius: 8px;
    width: min(500px, 90vw);
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 12px 48px rgba(0, 0, 0, 0.4);
    border: 1px solid var(--border-subtle);
  }

  .edit-modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .edit-modal-header h3 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .edit-modal-body {
    flex: 1;
    overflow-y: auto;
    padding: 20px;
  }

  .edit-modal-footer {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    padding: 16px 20px;
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-primary);
  }

  .form-group {
    margin-bottom: 16px;
  }

  .form-group label {
    display: block;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
    margin-bottom: 6px;
  }

  .form-group input[type='text'],
  .form-group select {
    width: 100%;
    padding: 9px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: 13px;
    transition: all 0.15s;
  }

  .form-group input[type='text']:focus,
  .form-group select:focus {
    outline: none;
    border-color: var(--ui-accent);
    box-shadow: 0 0 0 2px rgba(var(--color-primary-rgb, 59, 130, 246), 0.1);
  }

  .form-group input[type='text']::placeholder {
    color: var(--text-tertiary);
  }

  .checkbox-group label {
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    font-weight: 400;
  }

  .checkbox-group input[type='checkbox'] {
    cursor: pointer;
    width: 16px;
    height: 16px;
  }

  .type-hint {
    font-size: 12px;
    color: var(--text-secondary);
    margin-top: 8px;
    padding: 10px 12px;
    background: var(--bg-secondary);
    border-radius: 6px;
    border-left: 3px solid var(--color-warning);
  }

  .loading-state :global(svg),
  .actions-header :global(.spinner),
  .primary-btn :global(.spinner) {
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
