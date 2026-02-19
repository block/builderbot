<!--
  ProjectSection.svelte - A project header + session input + notes + branch cards

  Shows the project name, a prompt input for project-level sessions,
  project notes, repo controls, and all branch cards for this project.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { ChevronLeft, Trash2, Plus, Send, FileText, X } from 'lucide-svelte';
  import type { Project, Branch, WorkspaceStatus, ProjectNote } from '../../types';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome } from '../../navigation.svelte';
  import * as commands from '../../commands';
  import BranchCard from '../branches/BranchCard.svelte';
  import RemoteBranchCard from '../branches/RemoteBranchCard.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import GitHubRepoPicker from './GitHubRepoPicker.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
    repoLabelsById?: Map<string, { githubRepo: string; subpath: string | null }>;
    canAddRepo?: boolean;
    addRepoHint?: string | null;
    deleting?: boolean;
    safeToDelete?: boolean;
    deletingBranches?: Set<string>;
    worktreeErrors?: Map<string, string>;
    workspaceErrors?: Map<string, string>;
    detecting?: boolean;
    excludeRepos?: Set<string>;
    onDeleteProject?: () => void;
    onDeleteBranch?: (branchId: string) => void;
    onRenameBranch?: (branchId: string, branchName: string) => void;
    onWorkspaceStatusChange?: (branchId: string, status: WorkspaceStatus) => void;
    onRepoSelected?: (nameWithOwner: string, subpath?: string) => void;
    onRetryWorktree?: (branchId: string) => void;
  }

  let {
    project,
    branches,
    repoLabelsById = new Map(),
    canAddRepo = true,
    addRepoHint = null,
    deleting = false,
    safeToDelete = false,
    deletingBranches = new Set(),
    worktreeErrors = new Map(),
    workspaceErrors = new Map(),
    detecting = false,
    excludeRepos,
    onDeleteProject,
    onDeleteBranch,
    onRenameBranch,
    onWorkspaceStatusChange,
    onRepoSelected,
    onRetryWorktree,
  }: Props = $props();

  let sortedBranches = $derived([...branches].sort((a, b) => b.createdAt - a.createdAt));
  let addRepoDisabled = $derived(deleting || !canAddRepo);
  let addRepoTitle = $derived(
    deleting
      ? 'Project deletion in progress'
      : !canAddRepo && addRepoHint
        ? addRepoHint
        : 'Add repository to project'
  );

  let dropdownOpen = $state(false);
  let wrapperRef: HTMLDivElement | undefined = $state();

  function toggleDropdown() {
    dropdownOpen = !dropdownOpen;
  }

  function closeDropdown() {
    dropdownOpen = false;
  }

  function handleRepoSelected(nameWithOwner: string, subpath?: string) {
    dropdownOpen = false;
    onRepoSelected?.(nameWithOwner, subpath);
  }

  $effect(() => {
    if (!dropdownOpen) return;

    function onPointerDown(e: PointerEvent) {
      if (wrapperRef && !wrapperRef.contains(e.target as Node)) {
        dropdownOpen = false;
      }
    }

    window.addEventListener('pointerdown', onPointerDown);
    return () => window.removeEventListener('pointerdown', onPointerDown);
  });

  function repoLabelForBranch(
    branch: Branch
  ): { githubRepo: string; subpath: string | null } | null {
    const fallback = project.githubRepo
      ? { githubRepo: project.githubRepo, subpath: project.subpath ?? null }
      : null;
    if (!branch.projectRepoId) return fallback;
    return repoLabelsById.get(branch.projectRepoId) ?? fallback;
  }

  // ── Project session input ──────────────────────────────────────────────
  let promptText = $state('');
  /** Session IDs for running project sessions (all produce notes). */
  let activeSessionIds = $state<Set<string>>(new Set());

  async function handleSubmitPrompt() {
    const text = promptText.trim();
    if (!text) return;

    promptText = '';
    try {
      const response = await commands.startProjectSession(project.id, text);
      activeSessionIds = new Set([...activeSessionIds, response.sessionId]);
      // Reload notes immediately so the stub appears as "Generating note…"
      await loadProjectNotes();
    } catch (e) {
      console.error('[ProjectSection] Failed to start project session:', e);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmitPrompt();
    }
  }

  // ── Project notes ──────────────────────────────────────────────────────
  let projectNotes = $state<ProjectNote[]>([]);
  let deletingNoteIds = $state<Set<string>>(new Set());

  async function loadProjectNotes() {
    try {
      projectNotes = await commands.listProjectNotes(project.id);
    } catch (e) {
      console.error('[ProjectSection] Failed to load project notes:', e);
    }
  }

  async function handleDeleteNote(noteId: string) {
    deletingNoteIds = new Set([...deletingNoteIds, noteId]);
    try {
      await commands.deleteProjectNote(noteId);
      projectNotes = projectNotes.filter((n) => n.id !== noteId);
    } catch (e) {
      console.error('[ProjectSection] Failed to delete project note:', e);
    } finally {
      const next = new Set(deletingNoteIds);
      next.delete(noteId);
      deletingNoteIds = next;
    }
  }

  /** Notes sorted newest first, excluding empty ones (in-progress). */
  let displayNotes = $derived(
    [...projectNotes]
      .filter((n) => n.title.trim() || n.content.trim())
      .sort((a, b) => b.createdAt - a.createdAt)
  );

  /** Notes currently being generated (empty content, has session). */
  let generatingNotes = $derived(
    projectNotes.filter((n) => n.sessionId && !n.title.trim() && !n.content.trim())
  );

  function formatRelativeTime(timestampMs: number): string {
    const date = new Date(timestampMs);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHours = Math.floor(diffMins / 60);
    const diffDays = Math.floor(diffHours / 24);

    if (diffMins < 1) return 'just now';
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHours < 24) return `${diffHours}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return date.toLocaleDateString();
  }

  // ── Lifecycle ──────────────────────────────────────────────────────────

  onMount(() => {
    loadProjectNotes();

    let unlistenSession: (() => void) | undefined;
    listen<{ sessionId: string; status: string }>('session-status-changed', (event) => {
      if (activeSessionIds.has(event.payload.sessionId)) {
        if (event.payload.status === 'completed' || event.payload.status === 'failed') {
          const next = new Set(activeSessionIds);
          next.delete(event.payload.sessionId);
          activeSessionIds = next;
          // Refresh notes after session completes
          loadProjectNotes();
        }
      }
    }).then((unlisten) => {
      unlistenSession = unlisten;
    });

    return () => {
      unlistenSession?.();
    };
  });
</script>

<div class="project-section">
  <div class="project-header" class:deleting>
    <div class="project-info">
      <button class="back-button" onclick={goHome} title="Back to projects">
        <ChevronLeft size={16} />
      </button>
      <span class="project-name" title={projectDisplayName(project)}
        >{projectDisplayName(project)}</span
      >
      {#if deleting}
        <div class="deleting-status" role="status" aria-live="polite">
          <Spinner size={12} />
          <span>Deleting…</span>
        </div>
      {/if}
      {#if detecting}
        <div class="detecting-status">
          <Spinner size={12} />
          <span>Detecting actions</span>
        </div>
      {/if}
    </div>
    {#if !deleting}
      <div class="header-actions">
        <div class="add-repo-wrapper" bind:this={wrapperRef}>
          <button
            class="header-action-button"
            onclick={toggleDropdown}
            disabled={addRepoDisabled}
            title={addRepoTitle}
          >
            <span class="action-icon"><Plus size={12} /></span>
            Add Repo
          </button>
          {#if dropdownOpen}
            <div class="repo-picker-dropdown">
              <GitHubRepoPicker
                onSelect={handleRepoSelected}
                onBack={closeDropdown}
                {excludeRepos}
                showHeader={false}
              />
            </div>
          {/if}
        </div>
        <button
          class="header-action-button danger"
          class:safe-delete={safeToDelete}
          onclick={() => onDeleteProject?.()}
          title="Remove project"
        >
          <span class="trash-icon"><Trash2 size={14} /></span>
          Remove Project
        </button>
      </div>
    {/if}
  </div>

  <!-- Project session prompt -->
  <div class="project-prompt-section">
    <div class="prompt-input-wrapper">
      <textarea
        class="prompt-input"
        placeholder="Ask about this project…"
        bind:value={promptText}
        onkeydown={handleKeydown}
        rows={1}
      ></textarea>
      <button
        class="send-button"
        onclick={handleSubmitPrompt}
        disabled={!promptText.trim()}
        title="Start project session"
      >
        <Send size={14} />
      </button>
    </div>
  </div>

  <!-- Project notes -->
  {#if generatingNotes.length > 0 || displayNotes.length > 0}
    <div class="project-notes">
      <div class="notes-header">
        <FileText size={13} />
        <span>Project Notes</span>
      </div>
      {#each generatingNotes as _note (_note.id)}
        <div class="note-card generating">
          <div class="note-card-header">
            <span class="note-title">Generating note…</span>
            <Spinner size={12} />
          </div>
        </div>
      {/each}
      {#each displayNotes as note (note.id)}
        <div class="note-card" class:deleting={deletingNoteIds.has(note.id)}>
          <div class="note-card-header">
            <span class="note-title">{note.title || 'Untitled note'}</span>
            <div class="note-actions">
              <span class="note-time">{formatRelativeTime(note.createdAt)}</span>
              <button
                class="note-delete-btn"
                onclick={() => handleDeleteNote(note.id)}
                disabled={deletingNoteIds.has(note.id)}
                title="Delete note"
              >
                <X size={12} />
              </button>
            </div>
          </div>
          {#if note.content}
            <div class="note-content">{note.content}</div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  <div class="branches-list" class:deleting>
    {#each sortedBranches as branch (branch.id)}
      {#if branch.branchType === 'remote'}
        <RemoteBranchCard
          {branch}
          repoLabel={repoLabelForBranch(branch)}
          deleting={deletingBranches.has(branch.id)}
          workspaceError={workspaceErrors.get(branch.id)}
          onDelete={() => onDeleteBranch?.(branch.id)}
          onRename={(branchName) => onRenameBranch?.(branch.id, branchName)}
          onWorkspaceStatusChange={(status) => onWorkspaceStatusChange?.(branch.id, status)}
        />
      {:else}
        <BranchCard
          {branch}
          repoLabel={repoLabelForBranch(branch)}
          deleting={deletingBranches.has(branch.id)}
          worktreeError={worktreeErrors.get(branch.id)}
          onDelete={() => onDeleteBranch?.(branch.id)}
          onRename={(branchName) => onRenameBranch?.(branch.id, branchName)}
          onRetryWorktree={() => onRetryWorktree?.(branch.id)}
        />
      {/if}
    {/each}
  </div>
</div>

<style>
  .project-section {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .project-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px;
  }

  .project-info {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }

  .back-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background-color: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .back-button:hover {
    color: var(--text-primary);
    background-color: var(--ui-selection);
  }

  .project-name {
    font-size: var(--size-xl);
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: -0.01em;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .header-action-button {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background-color: transparent;
    border: none;
    border-radius: 8px;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .header-action-button:hover {
    color: var(--text-primary);
    background-color: var(--ui-selection);
  }

  .header-action-button:hover .action-icon {
    background-color: var(--border-emphasis);
  }

  .header-action-button:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .header-action-button:disabled:hover {
    color: var(--text-muted);
    background-color: transparent;
  }

  .header-action-button:disabled:hover .action-icon {
    background-color: var(--border-muted);
  }

  .action-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background-color: var(--border-muted);
    flex-shrink: 0;
  }

  .trash-icon {
    display: flex;
    align-items: center;
    color: var(--text-muted);
    transition: color 0.15s ease;
  }

  .header-action-button.danger:hover {
    color: var(--ui-danger);
  }

  .header-action-button.danger:hover .trash-icon {
    color: var(--ui-danger);
  }

  .header-action-button.safe-delete {
    color: var(--ui-danger);
    border: 1px solid var(--ui-danger);
  }

  .header-action-button.safe-delete .trash-icon {
    color: var(--ui-danger);
  }

  .detecting-status {
    height: 22px;
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 8px;
    padding: 0 10px;
    border-radius: 999px;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    line-height: 1;
    border: 1px solid var(--border-muted);
  }

  .deleting-status {
    height: 22px;
    display: flex;
    align-items: center;
    gap: 6px;
    margin-left: 8px;
    padding: 0 10px;
    border-radius: 999px;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 500;
    line-height: 1;
    border: 1px solid var(--border-muted);
  }

  /* ── Project prompt ──────────────────────────────────────────────────── */

  .project-prompt-section {
    padding: 0 4px;
  }

  .prompt-input-wrapper {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 8px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    background-color: var(--bg-primary);
    transition: border-color 0.15s ease;
  }

  .prompt-input-wrapper:focus-within {
    border-color: var(--border-emphasis);
  }

  .prompt-input {
    flex: 1;
    border: none;
    background: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    line-height: 1.5;
    resize: none;
    outline: none;
    min-height: 20px;
    max-height: 120px;
  }

  .prompt-input::placeholder {
    color: var(--text-faint);
  }

  .send-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 6px;
    background-color: var(--ui-accent);
    color: var(--bg-deepest);
    cursor: pointer;
    flex-shrink: 0;
    transition: all 0.15s ease;
  }

  .send-button:hover:not(:disabled) {
    background-color: var(--ui-accent-hover);
  }

  .send-button:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  /* ── Project notes ───────────────────────────────────────────────────── */

  .project-notes {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 0 4px;
  }

  .notes-header {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .note-card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 14px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background-color: var(--bg-primary);
    transition: all 0.15s ease;
  }

  .note-card:hover {
    border-color: var(--border-muted);
  }

  .note-card.generating {
    opacity: 0.7;
    border-color: var(--note-color, var(--ui-accent));
    border-style: dashed;
  }

  .note-card.deleting {
    opacity: 0.4;
    pointer-events: none;
  }

  .note-card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .note-title {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .note-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .note-time {
    font-size: var(--size-xs);
    color: var(--text-faint);
    white-space: nowrap;
  }

  .note-delete-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border: none;
    border-radius: 4px;
    background: none;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition: all 0.15s ease;
  }

  .note-card:hover .note-delete-btn {
    opacity: 1;
  }

  .note-delete-btn:hover {
    color: var(--ui-danger);
    background-color: var(--bg-hover);
  }

  .note-delete-btn:disabled {
    cursor: not-allowed;
    opacity: 0.3;
  }

  .note-content {
    font-size: var(--size-xs);
    color: var(--text-muted);
    line-height: 1.5;
    white-space: pre-wrap;
    overflow: hidden;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    -webkit-box-orient: vertical;
  }

  /* ── Branches list ───────────────────────────────────────────────────── */

  .branches-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .branches-list.deleting {
    opacity: 0.65;
    pointer-events: none;
  }

  .add-repo-wrapper {
    position: relative;
  }

  .repo-picker-dropdown {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    width: 420px;
    max-height: min(60vh, 420px);
    background-color: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    z-index: 100;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .repo-picker-dropdown :global(.repo-picker) {
    min-height: 0;
    flex: 1;
  }
</style>
