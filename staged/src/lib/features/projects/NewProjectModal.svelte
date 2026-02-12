<!--
  NewProjectModal.svelte - Create a new project from a repository

  Two-phase flow:
  1. FolderPickerModal to select a git repo
  2. Confirmation with optional subpath, then create
-->
<script lang="ts">
  import { X, GitBranch, Folder } from 'lucide-svelte';
  import type { Project } from '../../types';
  import * as commands from '../../commands';
  import FolderPickerModal from './FolderPickerModal.svelte';
  import { detectProjectActions } from '../actions/actions';
  import { listDirectory, type DirEntry } from '../../shared/files';

  interface Props {
    onCreated: (project: Project) => void;
    onDetecting: (projectId: string, detecting: boolean) => void;
    onClose: () => void;
  }

  let { onCreated, onDetecting, onClose }: Props = $props();

  let selectedRepo = $state<string | null>(null);
  let subpath = $state('');
  let importWorktrees = $state(false);
  let saving = $state(false);
  let error = $state<string | null>(null);

  // Subfolder autocomplete state
  let suggestions = $state<DirEntry[]>([]);
  let showDropdown = $state(false);
  let selectedIndex = $state(0);
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let subpathInputEl: HTMLInputElement | null = $state(null);

  /** Split subpath into the parent directory to list and trailing partial to filter by. */
  function getSubpathContext(value: string): { parentDir: string; partial: string } {
    const lastSlash = value.lastIndexOf('/');
    if (lastSlash === -1) return { parentDir: '', partial: value };
    return { parentDir: value.slice(0, lastSlash), partial: value.slice(lastSlash + 1) };
  }

  /** Load directory suggestions based on current subpath value. */
  async function loadSuggestions() {
    if (!selectedRepo) return;
    const { parentDir, partial } = getSubpathContext(subpath);
    const listPath = parentDir ? `${selectedRepo}/${parentDir}` : selectedRepo;

    try {
      const entries = await listDirectory(listPath);
      const dirs = entries.filter((e) => e.isDir);
      const lower = partial.toLowerCase();
      suggestions = lower ? dirs.filter((d) => d.name.toLowerCase().startsWith(lower)) : dirs;
      selectedIndex = 0;
      showDropdown = suggestions.length > 0;
    } catch {
      suggestions = [];
      showDropdown = false;
    }
  }

  /** Select a suggestion, filling the subpath and closing the dropdown. */
  function selectSuggestion(entry: DirEntry) {
    const { parentDir } = getSubpathContext(subpath);
    subpath = parentDir ? `${parentDir}/${entry.name}` : entry.name;
    showDropdown = false;
    suggestions = [];
    subpathInputEl?.focus();
  }

  function handleSubpathInput() {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(loadSuggestions, 150);
  }

  function handleSubpathFocus() {
    handleSubpathInput();
  }

  function handleSubpathBlur() {
    // Delay hiding so click on suggestion can register
    setTimeout(() => {
      showDropdown = false;
    }, 200);
  }

  function handleSubpathKeydown(e: KeyboardEvent) {
    if (!showDropdown) return;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      e.stopPropagation();
      selectedIndex = Math.min(selectedIndex + 1, suggestions.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      e.stopPropagation();
      selectedIndex = Math.max(selectedIndex - 1, 0);
    } else if (e.key === 'Enter' || e.key === 'Tab') {
      if (suggestions[selectedIndex]) {
        e.preventDefault();
        e.stopPropagation();
        selectSuggestion(suggestions[selectedIndex]);
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      showDropdown = false;
    }
  }

  function handleRepoSelected(path: string) {
    selectedRepo = path;
    error = null;
  }

  async function handleCreate() {
    if (!selectedRepo || saving) return;

    saving = true;
    error = null;

    try {
      const normalizedSubpath = subpath.trim().replace(/^\/+|\/+$/g, '') || undefined;
      const project = await commands.createProject(
        selectedRepo,
        normalizedSubpath,
        importWorktrees
      );

      // Auto-trigger action detection in background
      detectAndSaveActions(project.id).catch(() => {}); // Silent failure
      onDetecting(project.id, true);

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

  async function detectAndSaveActions(projectId: string) {
    try {
      const suggested = await detectProjectActions(projectId);

      // Save suggested actions
      for (let i = 0; i < suggested.length; i++) {
        const action = suggested[i];
        await commands.createProjectAction(
          projectId,
          action.name,
          action.command,
          action.actionType,
          i,
          action.autoCommit
        );
      }
    } finally {
      onDetecting(projectId, false);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    // Only handle keys when in confirmation phase (repo selected)
    if (!selectedRepo) return;

    if (e.key === 'Escape') {
      e.preventDefault();
      selectedRepo = null;
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      handleCreate();
    }
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      if (selectedRepo) {
        selectedRepo = null;
      } else {
        onClose();
      }
    }
  }

  function repoName(path: string): string {
    return path.split('/').pop() || path;
  }

  function formatPath(path: string): string {
    const home = '~';
    // We don't have homeDir here, but paths starting with /Users/<user>/
    // are common enough to detect
    const match = path.match(/^\/Users\/[^/]+\/(.*)/);
    if (match) return home + '/' + match[1];
    return path;
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if !selectedRepo}
  <FolderPickerModal onSelect={handleRepoSelected} {onClose} />
{:else}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="modal-backdrop"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onclick={handleBackdropClick}
    onkeydown={(e) => e.key === 'Escape' && (selectedRepo = null)}
  >
    <div class="modal">
      <div class="modal-header">
        <h2>New Project</h2>
        <button class="close-button" onclick={onClose}>
          <X size={18} />
        </button>
      </div>

      <div class="modal-body">
        <div class="repo-info">
          <GitBranch size={14} class="repo-info-icon" />
          <div class="repo-details">
            <span class="repo-name">{repoName(selectedRepo)}</span>
            <span class="repo-path">{formatPath(selectedRepo)}</span>
          </div>
          <button class="change-button" onclick={() => (selectedRepo = null)}>Change</button>
        </div>

        <div class="form-group">
          <label for="project-subpath">Subpath <span class="optional-label">(optional)</span></label
          >
          <div class="subpath-wrapper">
            <input
              bind:this={subpathInputEl}
              bind:value={subpath}
              id="project-subpath"
              type="text"
              placeholder="e.g., packages/frontend"
              disabled={saving}
              autocomplete="off"
              autocorrect="off"
              autocapitalize="off"
              spellcheck="false"
              oninput={handleSubpathInput}
              onfocus={handleSubpathFocus}
              onblur={handleSubpathBlur}
              onkeydown={handleSubpathKeydown}
            />
            {#if showDropdown && suggestions.length > 0}
              <div class="subpath-suggestions">
                {#each suggestions as entry, i (entry.path)}
                  <button
                    class="suggestion-item"
                    class:selected={i === selectedIndex}
                    onmousedown={(e) => {
                      e.preventDefault();
                      selectSuggestion(entry);
                    }}
                    onmouseenter={() => (selectedIndex = i)}
                  >
                    <Folder size={14} />
                    <span class="suggestion-name">{entry.name}</span>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
          <span class="help-text">
            For monorepos: subdirectory to use as working directory for AI sessions
          </span>
        </div>

        <label class="checkbox-group">
          <input type="checkbox" bind:checked={importWorktrees} disabled={saving} />
          <span class="checkbox-label">Import existing worktrees</span>
          <span class="checkbox-help">
            Detect and import git worktrees that already exist for this repo
          </span>
        </label>

        {#if error}
          <div class="error-message">{error}</div>
        {/if}

        <div class="actions">
          <button class="cancel-button" onclick={onClose} disabled={saving}>Cancel</button>
          <button class="create-button" onclick={handleCreate} disabled={saving}>
            {saving ? 'Creating...' : 'Create Project'}
          </button>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: 1000;
  }

  .modal {
    width: 460px;
    max-width: 90vw;
    background-color: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px;
    border-bottom: 1px solid var(--border-subtle);
    border-radius: 12px 12px 0 0;
  }

  .modal-header h2 {
    flex: 1;
    margin: 0;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .close-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .close-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .repo-info {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    background-color: var(--bg-hover);
    border-radius: 6px;
  }

  :global(.repo-info-icon) {
    color: var(--text-accent);
    flex-shrink: 0;
  }

  .repo-details {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .repo-name {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .repo-path {
    font-size: var(--size-xs);
    color: var(--text-muted);
    font-family: 'SF Mono', 'Menlo', monospace;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .change-button {
    padding: 4px 8px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    flex-shrink: 0;
    transition: all 0.15s ease;
  }

  .change-button:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .subpath-wrapper {
    position: relative;
  }

  .subpath-suggestions {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    margin-top: 4px;
    background-color: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    box-shadow: var(--shadow-elevated);
    max-height: 200px;
    overflow-y: auto;
    z-index: 10;
    padding: 4px 0;
  }

  .suggestion-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 6px 12px;
    background: none;
    border: none;
    text-align: left;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .suggestion-item:hover,
  .suggestion-item.selected {
    background-color: var(--bg-hover);
  }

  .suggestion-item :global(svg) {
    flex-shrink: 0;
    color: var(--text-muted);
  }

  .suggestion-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .form-group label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .optional-label {
    font-weight: 400;
    color: var(--text-faint);
  }

  .form-group input {
    width: 100%;
    padding: 10px 12px;
    background-color: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    font-size: var(--size-md);
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s;
    box-sizing: border-box;
  }

  .form-group input:focus {
    border-color: var(--ui-accent);
  }

  .form-group input:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .form-group input::placeholder {
    color: var(--text-faint);
  }

  .help-text {
    font-size: var(--size-xs);
    color: var(--text-muted);
  }

  .checkbox-group {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    cursor: pointer;
  }

  .checkbox-group input[type='checkbox'] {
    width: 16px;
    height: 16px;
    margin: 0;
    accent-color: var(--ui-accent);
    cursor: pointer;
  }

  .checkbox-group input[type='checkbox']:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .checkbox-label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
  }

  .checkbox-help {
    width: 100%;
    font-size: var(--size-xs);
    color: var(--text-muted);
    padding-left: 24px;
  }

  .error-message {
    padding: 10px 12px;
    background-color: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }

  .cancel-button {
    padding: 8px 16px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s;
  }

  .cancel-button:hover:not(:disabled) {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .cancel-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .create-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 16px;
    background-color: var(--ui-accent);
    border: none;
    border-radius: 6px;
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: background-color 0.15s;
  }

  .create-button:hover:not(:disabled) {
    background-color: var(--ui-accent-hover);
  }

  .create-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
