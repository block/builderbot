<!--
  BranchCard.svelte - Card display for a tracked branch

  Shows branch name, base branch, and a unified timeline of commits and notes.
  Commits are displayed oldest-first (newest at bottom) with the HEAD commit having a "Continue" option.
  Notes are interleaved by timestamp and styled differently.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import {
    GitBranch,
    Eye,
    Plus,
    Trash2,
    Loader2,
    Play,
    MessageSquare,
    FileText,
    GitCommit,
    ChevronDown,
    ChevronsUpDown,
    MoreVertical,
  } from 'lucide-svelte';
  import type { Branch, CommitInfo, BranchSession, BranchNote } from './services/branch';
  import * as branchService from './services/branch';
  import SessionViewerModal from './SessionViewerModal.svelte';
  import NewSessionModal from './NewSessionModal.svelte';
  import NewNoteModal from './NewNoteModal.svelte';
  import NoteViewerModal from './NoteViewerModal.svelte';
  import BaseBranchPickerModal from './BaseBranchPickerModal.svelte';

  interface Props {
    branch: Branch;
    /** Increment to force a data refresh */
    refreshKey?: number;
    onViewDiff?: () => void;
    onViewCommitDiff?: (sha: string) => void;
    onDelete?: () => void;
    onBranchUpdated?: (branch: Branch) => void;
  }

  let {
    branch,
    refreshKey = 0,
    onViewDiff,
    onViewCommitDiff,
    onDelete,
    onBranchUpdated,
  }: Props = $props();

  // Timeline item types
  type TimelineItem =
    | { type: 'commit'; commit: CommitInfo; session: BranchSession | null; isHead: boolean }
    | { type: 'note'; note: BranchNote }
    | { type: 'running-session'; session: BranchSession }
    | { type: 'generating-note'; note: BranchNote };

  // State
  let commits = $state<CommitInfo[]>([]);
  let notes = $state<BranchNote[]>([]);
  let runningSession = $state<BranchSession | null>(null);
  let generatingNote = $state<BranchNote | null>(null);
  let loading = $state(true);

  // Map of commit SHA to its associated session (for "View" button)
  let sessionsByCommit = $state<Map<string, BranchSession>>(new Map());

  // Unified timeline (computed from commits and notes)
  let timeline = $derived.by(() => {
    const items: TimelineItem[] = [];

    // Combine commits and notes, sort by timestamp (oldest first)
    const combined: Array<{ timestamp: number; item: TimelineItem }> = [];

    commits.forEach((commit, index) => {
      combined.push({
        timestamp: commit.timestamp,
        item: {
          type: 'commit',
          commit,
          session: sessionsByCommit.get(commit.sha) || null,
          isHead: index === 0 && !runningSession,
        },
      });
    });

    notes
      .filter((n) => n.status !== 'generating')
      .forEach((note) => {
        combined.push({
          // Note timestamps are in milliseconds, convert to seconds for comparison
          timestamp: Math.floor(note.createdAt / 1000),
          item: { type: 'note', note },
        });
      });

    // Sort by timestamp ascending (oldest first)
    combined.sort((a, b) => a.timestamp - b.timestamp);

    items.push(...combined.map((c) => c.item));

    // Add running session at bottom if exists
    if (runningSession) {
      items.push({ type: 'running-session', session: runningSession });
    }

    // Add generating note at bottom if exists
    if (generatingNote) {
      items.push({ type: 'generating-note', note: generatingNote });
    }

    return items;
  });

  // Session viewer modal state
  let showSessionViewer = $state(false);
  let viewingSession = $state<BranchSession | null>(null);
  let isViewingLive = $state(false);

  // Note viewer modal state
  let showNoteViewer = $state(false);
  let viewingNote = $state<BranchNote | null>(null);
  let isNoteGenerating = $state(false);

  // Continue session modal state
  let showContinueModal = $state(false);

  // New note modal state
  let showNewNoteModal = $state(false);

  // Base branch picker modal state
  let showBaseBranchPicker = $state(false);

  // Dropdown state
  let showNewDropdown = $state(false);
  let showMoreMenu = $state(false);

  // Delete note confirmation state
  let confirmingDeleteNoteId = $state<string | null>(null);

  // Load commits and running session on mount
  onMount(async () => {
    await loadData();
  });

  // Reload when refreshKey changes
  $effect(() => {
    if (refreshKey > 0) {
      loadData();
    }
  });

  async function loadData() {
    loading = true;
    try {
      const [commitsResult, sessionResult, notesResult, generatingNoteResult] = await Promise.all([
        branchService.getBranchCommits(branch.id),
        branchService.getRunningSession(branch.id),
        branchService.listBranchNotes(branch.id),
        branchService.getGeneratingNote(branch.id),
      ]);
      commits = commitsResult;
      runningSession = sessionResult;
      notes = notesResult;
      generatingNote = generatingNoteResult;

      // Load sessions for each commit (for "View" buttons)
      const sessionsMap = new Map<string, BranchSession>();
      await Promise.all(
        commitsResult.map(async (commit) => {
          const session = await branchService.getSessionForCommit(branch.id, commit.sha);
          if (session && session.aiSessionId) {
            sessionsMap.set(commit.sha, session);
          }
        })
      );
      sessionsByCommit = sessionsMap;
    } catch (e) {
      console.error('Failed to load branch data:', e);
    } finally {
      loading = false;
    }
  }

  // Format relative time
  function formatRelativeTime(timestamp: number): string {
    const date = new Date(timestamp * 1000); // Unix timestamp is in seconds
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

  // Format relative time from milliseconds
  function formatRelativeTimeMs(timestamp: number): string {
    return formatRelativeTime(Math.floor(timestamp / 1000));
  }

  function handleContinue() {
    showContinueModal = true;
  }

  function handleWatchSession() {
    if (runningSession?.aiSessionId) {
      viewingSession = runningSession;
      isViewingLive = true;
      showSessionViewer = true;
    }
  }

  function handleViewSession(session: BranchSession) {
    viewingSession = session;
    isViewingLive = false;
    showSessionViewer = true;
  }

  function closeSessionViewer() {
    showSessionViewer = false;
    viewingSession = null;
    isViewingLive = false;
  }

  function handleViewNote(note: BranchNote, generating: boolean = false) {
    viewingNote = note;
    isNoteGenerating = generating;
    showNoteViewer = true;
  }

  function closeNoteViewer() {
    showNoteViewer = false;
    viewingNote = null;
    isNoteGenerating = false;
  }

  function handleSessionStarted(branchSessionId: string, aiSessionId: string) {
    console.log('Session started:', { branchSessionId, aiSessionId });
    showContinueModal = false;
    loadData();
  }

  function handleNoteStarted(branchNoteId: string, aiSessionId: string) {
    console.log('Note started:', { branchNoteId, aiSessionId });
    showNewNoteModal = false;
    loadData();
  }

  function handleNewCommit() {
    showNewDropdown = false;
    showContinueModal = true;
  }

  function handleNewNote() {
    showNewDropdown = false;
    showNewNoteModal = true;
  }

  async function handleDeleteNote(noteId: string) {
    try {
      await branchService.deleteBranchNote(noteId);
      notes = notes.filter((n) => n.id !== noteId);
    } catch (e) {
      console.error('Failed to delete note:', e);
    }
    confirmingDeleteNoteId = null;
  }

  function toggleDropdown() {
    showNewDropdown = !showNewDropdown;
  }

  // Close dropdowns when clicking outside
  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (!target.closest('.new-dropdown-container')) {
      showNewDropdown = false;
    }
    if (!target.closest('.more-menu-container')) {
      showMoreMenu = false;
    }
  }

  function toggleMoreMenu(e: MouseEvent) {
    e.stopPropagation();
    showMoreMenu = !showMoreMenu;
  }

  function handleViewDiff() {
    showMoreMenu = false;
    onViewDiff?.();
  }

  function handleDeleteFromMenu() {
    showMoreMenu = false;
    onDelete?.();
  }

  // Handle base branch change
  async function handleBaseBranchChanged(newBaseBranch: string) {
    showBaseBranchPicker = false;
    // Notify parent so it can update its branch state
    const updatedBranch = { ...branch, baseBranch: newBaseBranch };
    onBranchUpdated?.(updatedBranch);
    // Reload data since commits may have changed
    await loadData();
  }

  // Format base branch for display (strip origin/ prefix if present)
  function formatBaseBranch(baseBranch: string): string {
    return baseBranch.replace(/^origin\//, '');
  }
</script>

<svelte:window on:click={handleClickOutside} />

<div class="branch-card">
  <div class="card-header">
    <div class="branch-info">
      <GitBranch size={16} class="branch-icon" />
      <span class="branch-name">{branch.branchName}</span>
      <span class="branch-separator">›</span>
      <button
        class="base-branch-name"
        onclick={() => (showBaseBranchPicker = true)}
        title="Change base branch"
      >
        {formatBaseBranch(branch.baseBranch)}
        <ChevronsUpDown size={12} class="base-branch-chevron" />
      </button>
    </div>
    <div class="more-menu-container">
      <button class="more-button" onclick={toggleMoreMenu} title="More options">
        <MoreVertical size={16} />
      </button>
      {#if showMoreMenu}
        <div class="more-menu">
          <button class="more-menu-item" onclick={handleViewDiff}>
            <Eye size={14} />
            View Diff
          </button>
          <button class="more-menu-item danger" onclick={handleDeleteFromMenu}>
            <Trash2 size={14} />
            Delete
          </button>
        </div>
      {/if}
    </div>
  </div>

  <div class="card-content">
    {#if loading}
      <div class="loading">
        <Loader2 size={14} class="spinner" />
        <span>Loading...</span>
      </div>
    {:else if timeline.length === 0}
      <p class="no-items">No commits or notes yet</p>
    {:else}
      <div class="timeline">
        {#each timeline as item, index (item.type === 'commit' ? item.commit.sha : item.type === 'note' ? item.note.id : item.type === 'running-session' ? 'running' : 'generating')}
          {#if item.type === 'generating-note'}
            <!-- Generating note skeleton -->
            <button
              class="timeline-row skeleton-row"
              onclick={() => handleViewNote(item.note, true)}
            >
              <div class="timeline-marker">
                <Loader2 size={12} class="spinner note-spinner" />
                {#if index < timeline.length - 1}
                  <div class="timeline-line"></div>
                {/if}
              </div>
              <div class="timeline-content">
                <div class="timeline-icon note-icon generating">
                  <FileText size={12} />
                </div>
                <div class="timeline-info">
                  <span class="timeline-title skeleton-title">{item.note.title}</span>
                  <div class="timeline-meta">
                    <span class="skeleton-meta">generating...</span>
                  </div>
                </div>
              </div>
              <div class="watch-button">
                <MessageSquare size={12} />
                Watch
              </div>
            </button>
          {:else if item.type === 'running-session'}
            <!-- Running session skeleton -->
            <button class="timeline-row skeleton-row" onclick={handleWatchSession}>
              <div class="timeline-marker">
                <Loader2 size={12} class="spinner commit-spinner" />
                {#if index < timeline.length - 1}
                  <div class="timeline-line"></div>
                {/if}
              </div>
              <div class="timeline-content">
                <div class="timeline-icon commit-icon generating">
                  <GitCommit size={12} />
                </div>
                <div class="timeline-info">
                  <span class="timeline-title skeleton-title">{item.session.prompt}</span>
                  <div class="timeline-meta">
                    <span class="skeleton-meta">generating...</span>
                  </div>
                </div>
              </div>
              <div class="watch-button">
                <MessageSquare size={12} />
                Watch
              </div>
            </button>
          {:else if item.type === 'commit'}
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div
              class="timeline-row commit-row"
              class:is-head={item.isHead}
              class:has-session={!!item.session}
              onclick={() => {
                if (item.session) handleViewSession(item.session);
              }}
            >
              <div class="timeline-marker">
                {#if item.isHead}
                  <div class="head-marker"></div>
                {:else}
                  <div class="timeline-dot commit-dot"></div>
                {/if}
                {#if index < timeline.length - 1}
                  <div class="timeline-line"></div>
                {/if}
              </div>
              <div class="timeline-content">
                <div class="timeline-icon commit-icon">
                  <GitCommit size={12} />
                </div>
                <div class="timeline-info">
                  <span class="timeline-title">{item.commit.subject}</span>
                  <div class="timeline-meta">
                    <span class="commit-sha">{item.commit.shortSha}</span>
                    <span class="timeline-time">{formatRelativeTime(item.commit.timestamp)}</span>
                  </div>
                </div>
              </div>
              <div class="timeline-actions">
                {#if item.isHead}
                  <button
                    class="action-btn"
                    onclick={(e) => {
                      e.stopPropagation();
                      handleContinue();
                    }}
                  >
                    <Play size={12} />
                    Continue
                  </button>
                {/if}
                <button
                  class="action-btn action-btn-icon"
                  onclick={(e) => {
                    e.stopPropagation();
                    onViewCommitDiff?.(item.commit.sha);
                  }}
                  title="View diff"
                >
                  <Eye size={12} />
                </button>
              </div>
            </div>
          {:else if item.type === 'note'}
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <div
              class="timeline-row note-row"
              role="button"
              tabindex="0"
              onclick={() => {
                if (confirmingDeleteNoteId === item.note.id) {
                  confirmingDeleteNoteId = null;
                } else {
                  handleViewNote(item.note);
                }
              }}
              onkeydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  if (confirmingDeleteNoteId === item.note.id) {
                    confirmingDeleteNoteId = null;
                  } else {
                    handleViewNote(item.note);
                  }
                }
              }}
            >
              <div class="timeline-marker">
                <div class="timeline-dot note-dot"></div>
                {#if index < timeline.length - 1}
                  <div class="timeline-line"></div>
                {/if}
              </div>
              <div class="timeline-content">
                <div class="timeline-icon note-icon">
                  <FileText size={12} />
                </div>
                <div class="timeline-info">
                  <span class="timeline-title">{item.note.title}</span>
                  <div class="timeline-meta">
                    <span class="timeline-time">{formatRelativeTimeMs(item.note.createdAt)}</span>
                  </div>
                </div>
              </div>
              <div class="timeline-actions">
                {#if confirmingDeleteNoteId === item.note.id}
                  <div class="delete-confirm">
                    <button
                      class="delete-confirm-btn"
                      onclick={(e) => {
                        e.stopPropagation();
                        handleDeleteNote(item.note.id);
                      }}
                    >
                      Delete
                    </button>
                    <button
                      class="delete-cancel-btn"
                      onclick={(e) => {
                        e.stopPropagation();
                        confirmingDeleteNoteId = null;
                      }}
                    >
                      Cancel
                    </button>
                  </div>
                {:else}
                  <button
                    class="note-delete"
                    onclick={(e) => {
                      e.stopPropagation();
                      confirmingDeleteNoteId = item.note.id;
                    }}
                    title="Delete note"
                  >
                    <Trash2 size={12} />
                  </button>
                {/if}
              </div>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <div class="card-footer">
    <div class="new-dropdown-container">
      <button
        class="new-button"
        onclick={toggleDropdown}
        disabled={!!runningSession || !!generatingNote}
      >
        <Plus size={14} />
        New
        <ChevronDown size={12} class={showNewDropdown ? 'chevron open' : 'chevron'} />
      </button>
      {#if showNewDropdown}
        <div class="dropdown-menu">
          <button class="dropdown-item" onclick={handleNewCommit}>
            <GitCommit size={14} />
            New Commit
          </button>
          <button class="dropdown-item" onclick={handleNewNote}>
            <FileText size={14} />
            New Note
          </button>
        </div>
      {/if}
    </div>
  </div>
</div>

<!-- Session viewer modal -->
{#if showSessionViewer && viewingSession?.aiSessionId}
  <SessionViewerModal
    sessionId={viewingSession.aiSessionId}
    title={viewingSession.prompt}
    isLive={isViewingLive}
    onClose={closeSessionViewer}
  />
{/if}

<!-- Note viewer modal -->
{#if showNoteViewer && viewingNote}
  <NoteViewerModal note={viewingNote} isLive={isNoteGenerating} onClose={closeNoteViewer} />
{/if}

<!-- Continue session modal -->
{#if showContinueModal}
  <NewSessionModal
    {branch}
    {commits}
    {sessionsByCommit}
    {notes}
    onClose={() => (showContinueModal = false)}
    onSessionStarted={handleSessionStarted}
  />
{/if}

<!-- New note modal -->
{#if showNewNoteModal}
  <NewNoteModal
    {branch}
    {commits}
    {sessionsByCommit}
    {notes}
    onClose={() => (showNewNoteModal = false)}
    onNoteStarted={handleNoteStarted}
  />
{/if}

<!-- Base branch picker modal -->
{#if showBaseBranchPicker}
  <BaseBranchPickerModal
    {branch}
    onClose={() => (showBaseBranchPicker = false)}
    onSelected={handleBaseBranchChanged}
  />
{/if}

<style>
  .branch-card {
    display: flex;
    flex-direction: column;
    background-color: var(--bg-primary);
    border-radius: 8px;
    overflow: hidden;
  }

  /* Header */
  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  :global(.branch-icon) {
    color: var(--status-renamed);
  }

  .branch-name {
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .branch-separator {
    color: var(--text-faint);
    font-size: var(--size-md);
  }

  .base-branch-name {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-muted);
    background: transparent;
    border: none;
    padding: 0;
    cursor: pointer;
    transition: color 0.15s ease;
  }

  .base-branch-name:hover {
    color: var(--text-primary);
  }

  :global(.base-branch-chevron) {
    color: var(--text-faint);
    flex-shrink: 0;
    position: relative;
    top: 1px;
  }

  .base-branch-name:hover :global(.base-branch-chevron) {
    color: var(--text-muted);
  }

  /* More menu */
  .more-menu-container {
    position: relative;
  }

  .more-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .more-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .more-menu {
    position: absolute;
    top: 100%;
    right: 0;
    margin-top: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: hidden;
    z-index: 100;
    min-width: 120px;
  }

  .more-menu-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    text-align: left;
  }

  .more-menu-item:hover {
    background-color: var(--bg-hover);
  }

  .more-menu-item :global(svg) {
    color: var(--text-muted);
  }

  .more-menu-item.danger:hover {
    background-color: var(--ui-danger-bg);
    color: var(--ui-danger);
  }

  .more-menu-item.danger:hover :global(svg) {
    color: var(--ui-danger);
  }

  /* Content */
  .card-content {
    padding: 12px 16px;
    min-height: 60px;
  }

  .loading {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-muted);
    font-size: var(--size-sm);
  }

  .no-items {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-faint);
    font-style: italic;
  }

  /* Timeline */
  .timeline {
    display: flex;
    flex-direction: column;
  }

  .timeline-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 6px 8px;
    margin: 0 -8px;
    background: transparent;
    border: none;
    border-radius: 6px;
    text-align: left;
    width: calc(100% + 16px);
    position: relative;
    transition: background-color 0.15s ease;
  }

  .timeline-row.commit-row.has-session,
  .timeline-row.note-row,
  .timeline-row.skeleton-row {
    cursor: pointer;
  }

  .timeline-row.commit-row.has-session:hover,
  .timeline-row.note-row:hover,
  .timeline-row.skeleton-row:hover {
    background-color: var(--bg-hover);
  }

  .timeline-marker {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: 12px;
    padding-top: 6px;
  }

  .head-marker {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background-color: var(--ui-accent);
    box-shadow:
      0 0 0 2px var(--bg-primary),
      0 0 0 3px var(--ui-accent);
  }

  .timeline-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }

  .commit-dot {
    background-color: var(--border-emphasis);
  }

  .note-dot {
    background-color: var(--text-accent);
  }

  .timeline-line {
    flex: 1;
    width: 2px;
    min-height: 20px;
    background-color: var(--border-subtle);
    margin-top: 4px;
  }

  .timeline-content {
    flex: 1;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    min-width: 0;
  }

  .timeline-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .commit-icon {
    background-color: rgba(63, 185, 80, 0.15);
    color: var(--status-added);
  }

  .commit-icon.generating {
    background-color: rgba(63, 185, 80, 0.1);
    color: var(--status-added);
  }

  .note-icon {
    background-color: rgba(88, 166, 255, 0.15);
    color: var(--text-accent);
  }

  .note-icon.generating {
    background-color: rgba(88, 166, 255, 0.1);
    color: var(--text-accent);
  }

  .timeline-info {
    flex: 1;
    min-width: 0;
  }

  .timeline-title {
    display: block;
    font-size: var(--size-sm);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .skeleton-title {
    color: var(--text-muted);
    font-style: italic;
  }

  .timeline-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 2px;
  }

  .commit-sha {
    font-size: var(--size-xs);
    font-family: 'SF Mono', 'Menlo', 'Monaco', 'Courier New', monospace;
    color: var(--text-faint);
  }

  .timeline-time {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }

  .skeleton-meta {
    font-size: var(--size-xs);
    color: var(--text-faint);
    background: linear-gradient(
      90deg,
      var(--bg-hover) 25%,
      var(--bg-primary) 50%,
      var(--bg-hover) 75%
    );
    background-size: 200% 100%;
    animation: shimmer 1.5s infinite;
    border-radius: 4px;
    padding: 0 4px;
  }

  .timeline-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }

  .action-btn {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    background-color: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .action-btn:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    background-color: var(--bg-hover);
  }

  .action-btn-icon {
    padding: 4px 6px;
    opacity: 0;
  }

  .commit-row:hover .action-btn-icon {
    opacity: 1;
  }

  .note-delete {
    position: absolute;
    right: 8px;
    top: 50%;
    transform: translateY(-50%);
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    background: none;
    border: none;
    border-radius: 4px;
    color: var(--text-faint);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.15s ease,
      background-color 0.15s ease,
      color 0.15s ease;
  }

  .note-row:hover .note-delete {
    opacity: 1;
  }

  .note-delete:hover {
    background-color: var(--bg-hover);
    color: var(--ui-danger);
  }

  /* Delete note confirmation inline */
  .delete-confirm {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .delete-confirm-btn {
    padding: 2px 8px;
    background: var(--ui-danger);
    border: none;
    border-radius: 3px;
    color: white;
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .delete-confirm-btn:hover {
    background: var(--ui-danger-hover, #c53030);
  }

  .delete-cancel-btn {
    padding: 2px 8px;
    background: none;
    border: none;
    border-radius: 3px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-family: inherit;
    cursor: pointer;
    transition:
      background-color 0.1s,
      color 0.1s;
  }

  .delete-cancel-btn:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .watch-button {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 10px;
    background-color: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    transition: all 0.15s ease;
    flex-shrink: 0;
  }

  .skeleton-row:hover .watch-button {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
  }

  /* Footer */
  .card-footer {
    display: flex;
    justify-content: flex-end;
    padding: 12px 16px;
    border-top: 1px solid var(--border-subtle);
  }

  .new-dropdown-container {
    position: relative;
  }

  .new-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background-color: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .new-button:hover:not(:disabled) {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
    background-color: var(--bg-hover);
  }

  .new-button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  :global(.chevron) {
    transition: transform 0.15s ease;
  }

  :global(.chevron.open) {
    transform: rotate(180deg);
  }

  .dropdown-menu {
    position: absolute;
    bottom: 100%;
    right: 0;
    margin-bottom: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
    overflow: hidden;
    z-index: 100;
    min-width: 140px;
  }

  .dropdown-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    text-align: left;
  }

  .dropdown-item:hover {
    background-color: var(--bg-hover);
  }

  .dropdown-item :global(svg) {
    color: var(--text-muted);
  }

  /* Spinner animations */
  :global(.spinner) {
    animation: spin 1s linear infinite;
  }

  :global(.commit-spinner) {
    color: var(--status-added);
  }

  :global(.note-spinner) {
    color: var(--text-accent);
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes shimmer {
    0% {
      background-position: 200% 0;
    }
    100% {
      background-position: -200% 0;
    }
  }
</style>
