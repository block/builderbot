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
    FileDiff,
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
    ExternalLink,
    AlertCircle,
  } from 'lucide-svelte';
  import type { Branch, CommitInfo, BranchSession, BranchNote, OpenerApp } from './services/branch';
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

  // Open in... button state
  let openerApps = $state<OpenerApp[]>([]);
  let openersLoaded = $state(false);
  let showOpenInDropdown = $state(false);

  // Delete note confirmation state
  let confirmingDeleteNoteId = $state<string | null>(null);

  // Delete commit confirmation state
  let confirmingDeleteCommitSha = $state<string | null>(null);
  let commitsToDeleteCount = $state(0);
  let deletingCommit = $state(false);

  // Track if the running session is actually alive (AI session still connected)
  let isRunningSessionAlive = $state(true);

  // Load commits and running session on mount
  onMount(async () => {
    await loadData();
    // Load available openers (shared across all cards via cache)
    if (!openersLoaded) {
      branchService.getAvailableOpeners().then((apps) => {
        openerApps = apps;
        openersLoaded = true;
      });
    }
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

      // Check if running session is actually alive
      if (sessionResult?.aiSessionId) {
        try {
          isRunningSessionAlive = await branchService.isSessionAlive(sessionResult.aiSessionId);
        } catch (e) {
          // If we can't check, assume it's dead (safer - allows recovery)
          console.warn('Failed to check session alive status:', e);
          isRunningSessionAlive = false;
        }
      } else {
        isRunningSessionAlive = true;
      }

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

  function handleNoteStarted(branchNoteId: string, aiSessionId: string, provider: string) {
    console.log('Note started:', { branchNoteId, aiSessionId, provider });
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

  // Start delete commit confirmation - calculate how many commits will be removed
  function startDeleteCommit(commitSha: string) {
    // Find the index of this commit (commits are newest-first)
    const commitIndex = commits.findIndex((c) => c.sha === commitSha);
    if (commitIndex === -1) return;

    // All commits from index 0 to commitIndex (inclusive) will be deleted
    // because they are newer or equal to the target commit
    commitsToDeleteCount = commitIndex + 1;
    confirmingDeleteCommitSha = commitSha;
  }

  async function handleDeleteCommit() {
    if (!confirmingDeleteCommitSha) return;

    // Find the session for this commit
    const session = sessionsByCommit.get(confirmingDeleteCommitSha);
    if (!session) {
      console.error('No session found for commit:', confirmingDeleteCommitSha);
      confirmingDeleteCommitSha = null;
      return;
    }

    deletingCommit = true;
    try {
      await branchService.deleteBranchSessionAndCommit(session.id);
      // Reload data to reflect the changes
      await loadData();
    } catch (e) {
      console.error('Failed to delete commit:', e);
    } finally {
      deletingCommit = false;
      confirmingDeleteCommitSha = null;
      commitsToDeleteCount = 0;
    }
  }

  function cancelDeleteCommit() {
    confirmingDeleteCommitSha = null;
    commitsToDeleteCount = 0;
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
    if (!target.closest('.open-in-container')) {
      showOpenInDropdown = false;
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

  async function handleOpenInApp(appId: string) {
    showOpenInDropdown = false;
    try {
      await branchService.openInApp(branch.worktreePath, appId);
    } catch (e) {
      console.error('Failed to open in app:', e);
    }
  }

  function toggleOpenInDropdown(e: MouseEvent) {
    e.stopPropagation();
    showOpenInDropdown = !showOpenInDropdown;
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

  // Handle discarding a stuck session
  async function handleDiscardSession() {
    if (!runningSession) return;
    try {
      await branchService.cancelBranchSession(runningSession.id);
      runningSession = null;
      isRunningSessionAlive = true;
      await loadData();
    } catch (e) {
      console.error('Failed to discard session:', e);
    }
  }

  // Handle restarting a stuck session
  async function handleRestartSession() {
    if (!runningSession) return;
    try {
      // Use the stored user prompt for restart
      const fullPrompt = runningSession.prompt;
      const result = await branchService.restartBranchSession(runningSession.id, fullPrompt);
      console.log('Session restarted:', result);
      isRunningSessionAlive = true;
      await loadData();
    } catch (e) {
      console.error('Failed to restart session:', e);
    }
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
    <div class="header-actions">
      <div class="more-menu-container">
        <button class="more-button" onclick={toggleMoreMenu} title="More options">
          <MoreVertical size={16} />
        </button>
        {#if showMoreMenu}
          <div class="more-menu">
            <button class="more-menu-item" onclick={handleViewDiff}>
              <FileDiff size={14} />
              View Diff
            </button>
            <button class="more-menu-item danger" onclick={handleDeleteFromMenu}>
              <Trash2 size={14} />
              Delete
            </button>
          </div>
        {/if}
      </div>
      <!-- Open in... button -->
      {#if openerApps.length > 0}
        <div class="open-in-container">
          <button class="open-button" onclick={toggleOpenInDropdown} title="Open in...">
            <ExternalLink size={13} />
            Open
            <ChevronDown size={12} />
          </button>
          {#if showOpenInDropdown}
            <div class="open-in-dropdown">
              {#each openerApps as app (app.id)}
                <button class="open-in-item" onclick={() => handleOpenInApp(app.id)}>
                  {app.name}
                </button>
              {/each}
            </div>
          {/if}
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
                <div class="timeline-icon note-icon">
                  <Loader2 size={12} class="spinner" />
                </div>
                {#if index < timeline.length - 1}
                  <div class="timeline-line"></div>
                {/if}
              </div>
              <div class="timeline-content">
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
            {#if isRunningSessionAlive}
              <button class="timeline-row skeleton-row" onclick={handleWatchSession}>
                <div class="timeline-marker">
                  <div class="timeline-icon commit-icon">
                    <Loader2 size={12} class="spinner" />
                  </div>
                  {#if index < timeline.length - 1}
                    <div class="timeline-line"></div>
                  {/if}
                </div>
                <div class="timeline-content">
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
            {:else}
              <!-- Stuck session - show recovery options -->
              <div class="timeline-row skeleton-row stuck-session-row">
                <div class="timeline-marker">
                  <div
                    class="timeline-icon stuck-icon"
                    title="Session was interrupted before completing"
                  >
                    <AlertCircle size={12} />
                  </div>
                  {#if index < timeline.length - 1}
                    <div class="timeline-line"></div>
                  {/if}
                </div>
                <div class="timeline-content">
                  <div class="timeline-info">
                    <span class="timeline-title skeleton-title">{item.session.prompt}</span>
                  </div>
                </div>
                <div class="stuck-actions">
                  <button
                    class="stuck-btn"
                    onclick={(e) => {
                      e.stopPropagation();
                      handleRestartSession();
                    }}
                    title="Try again with the same prompt"
                  >
                    <Play size={12} />
                  </button>
                  <button
                    class="stuck-btn stuck-btn-danger"
                    onclick={(e) => {
                      e.stopPropagation();
                      handleDiscardSession();
                    }}
                    title="Remove this interrupted session"
                  >
                    <Trash2 size={12} />
                  </button>
                </div>
              </div>
            {/if}
          {:else if item.type === 'commit'}
            <div class="timeline-row commit-row" class:is-head={item.isHead}>
              <div class="timeline-marker">
                <div class="timeline-icon commit-icon">
                  <GitCommit size={12} />
                </div>
                {#if index < timeline.length - 1}
                  <div class="timeline-line"></div>
                {/if}
              </div>
              <div class="timeline-content">
                <div class="timeline-info">
                  <span class="timeline-title">{item.commit.subject}</span>
                  <div class="timeline-meta">
                    <span class="commit-sha">{item.commit.shortSha}</span>
                    <span class="timeline-time">{formatRelativeTime(item.commit.timestamp)}</span>
                  </div>
                </div>
              </div>
              <div class="timeline-actions">
                {#if confirmingDeleteCommitSha === item.commit.sha}
                  <div class="delete-confirm">
                    {#if commitsToDeleteCount > 1}
                      <span class="delete-warning">Will delete {commitsToDeleteCount} commits</span>
                    {/if}
                    <button
                      class="action-btn action-btn-danger"
                      disabled={deletingCommit}
                      onclick={() => handleDeleteCommit()}
                    >
                      {deletingCommit ? 'Deleting...' : 'Delete'}
                    </button>
                    <button
                      class="action-btn"
                      disabled={deletingCommit}
                      onclick={() => cancelDeleteCommit()}
                    >
                      Cancel
                    </button>
                  </div>
                {:else}
                  {#if item.session}
                    <button
                      class="action-btn action-btn-icon action-btn-hover"
                      onclick={() => item.session && handleViewSession(item.session)}
                      title="View session"
                    >
                      <MessageSquare size={12} />
                    </button>
                  {/if}
                  <button
                    class="action-btn action-btn-icon action-btn-hover"
                    onclick={() => onViewCommitDiff?.(item.commit.sha)}
                    title="View diff"
                  >
                    <FileDiff size={12} />
                  </button>
                  {#if item.session}
                    <button
                      class="action-btn action-btn-icon action-btn-hover commit-delete"
                      onclick={() => startDeleteCommit(item.commit.sha)}
                      title="Delete commit"
                    >
                      <Trash2 size={12} />
                    </button>
                  {/if}
                  {#if item.isHead}
                    <button class="action-btn action-btn-hover" onclick={() => handleContinue()}>
                      <Play size={12} />
                      Continue
                    </button>
                  {/if}
                {/if}
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
                <div class="timeline-icon note-icon">
                  <FileText size={12} />
                </div>
                {#if index < timeline.length - 1}
                  <div class="timeline-line"></div>
                {/if}
              </div>
              <div class="timeline-content">
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
                      class="action-btn action-btn-danger"
                      onclick={(e) => {
                        e.stopPropagation();
                        handleDeleteNote(item.note.id);
                      }}
                    >
                      Delete
                    </button>
                    <button
                      class="action-btn"
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
                    class="action-btn action-btn-icon action-btn-hover"
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
    onClose={() => (showContinueModal = false)}
    onSessionStarted={handleSessionStarted}
  />
{/if}

<!-- New note modal -->
{#if showNewNoteModal}
  <NewNoteModal
    {branch}
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

  /* Header actions */
  .header-actions {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  /* Open in... button */
  .open-in-container {
    position: relative;
  }

  .open-button {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-muted);
    font-size: var(--size-xs);
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .open-button:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .open-in-dropdown {
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
    min-width: 140px;
  }

  .open-in-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 14px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    text-align: left;
    white-space: nowrap;
  }

  .open-in-item:hover {
    background-color: var(--bg-hover);
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

  .timeline-row.note-row,
  .timeline-row.skeleton-row {
    cursor: pointer;
  }

  .timeline-row.note-row:hover,
  .timeline-row.skeleton-row:hover {
    background-color: var(--bg-hover);
  }

  .timeline-marker {
    display: flex;
    flex-direction: column;
    align-items: center;
    width: 20px;
    flex-shrink: 0;
  }

  .timeline-line {
    flex: 1;
    width: 2px;
    min-height: 16px;
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
    background-color: var(--bg-hover);
  }

  .timeline-icon.commit-icon {
    color: var(--status-added);
  }

  .timeline-icon.note-icon {
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
    visibility: hidden;
  }

  .action-btn-hover {
    visibility: hidden;
  }

  .commit-row:hover .action-btn-icon,
  .commit-row:hover .action-btn-hover,
  .note-row:hover .action-btn-icon,
  .note-row:hover .action-btn-hover {
    visibility: visible;
  }

  .action-btn-danger {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  .action-btn-danger:hover {
    background-color: var(--ui-danger);
    border-color: var(--ui-danger);
    color: white;
  }

  .commit-delete:hover {
    border-color: var(--ui-danger) !important;
    color: var(--ui-danger) !important;
  }

  /* Delete confirmation inline */
  .delete-confirm {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .delete-warning {
    font-size: var(--size-xs);
    color: var(--ui-danger);
    margin-right: 4px;
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

  /* Stuck session styles */
  .timeline-icon.stuck-icon {
    background-color: var(--ui-danger-bg);
    color: var(--ui-danger);
  }

  .stuck-actions {
    display: flex;
    gap: 4px;
    flex-shrink: 0;
  }

  .stuck-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .stuck-btn:hover {
    border-color: var(--ui-accent);
    color: var(--ui-accent);
  }

  .stuck-btn.stuck-btn-danger:hover {
    border-color: var(--status-error);
    color: var(--status-error);
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
