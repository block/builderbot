<!--
  ProjectSection.svelte - A project header + session input + notes + branch cards

  Shows the project name, a prompt input for project-level sessions,
  project notes, repo controls, and all branch cards for this project.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { untrack } from 'svelte';
  import {
    ChevronLeft,
    Trash2,
    Plus,
    Send,
    FileText,
    CircleCheck,
    CirclePause,
    Pause,
    AlertCircle,
    Cloud,
    Paperclip,
    X,
    ImagePlus,
  } from 'lucide-svelte';
  import type {
    Project,
    ProjectRepo,
    Branch,
    WorkspaceStatus,
    ProjectNote,
    HashtagItem,
  } from '../../types';
  import { projectDisplayName } from '../../shared/utils';
  import { goHome } from '../layout/navigation.svelte';
  import * as commands from '../../api/commands';
  import HashtagInput from '../sessions/HashtagInput.svelte';
  import { buildProjectHashtagItems } from '../sessions/hashtagItems';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import BranchCard from '../branches/BranchCard.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import AddRepoModal from './AddRepoModal.svelte';
  import type { RepoSelection as RepoPickerSelection } from '../../shared/githubUrl';
  import TimelineRow from '../timeline/TimelineRow.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import SessionModal from '../sessions/SessionModal.svelte';
  import AgentSelector from '../agents/AgentSelector.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { getPreferredAgentForProject } from '../settings/preferences.svelte';
  import { subscribeDragDrop } from '../branches/dragDrop';
  import { isImageFile } from '../branches/branchCardHelpers';
  import { createImage, createImageFromData, deleteImage, getImageData } from '../../api/commands';
  import { formatRelativeTime, minuteNow } from '../../shared/relativeTime.svelte';
  import { createLiveSessionHints } from '../timeline/liveSessionHints';

  interface Props {
    project: Project;
    branches: Branch[];
    reposById?: Map<string, ProjectRepo>;
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
    onRepoSelected?: (selection: RepoPickerSelection) => void;
    onRetryWorktree?: (branchId: string) => void;
    onResumeWorkspace?: (workspaceName: string) => void;
    onDismissReason?: (projectRepoId: string) => void;
  }

  let {
    project,
    branches,
    reposById = new Map(),
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
    onRepoSelected,
    onRetryWorktree,
    onResumeWorkspace,
    onDismissReason,
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

  // For remote projects, derive workspace info from any branch (they all share the same workspace)
  let workspaceBranch = $derived(
    project.location === 'remote' ? (branches.find((b) => b.workspaceStatus) ?? null) : null
  );
  let projectWorkspaceStatus = $derived<WorkspaceStatus | null>(
    workspaceBranch?.workspaceStatus ?? null
  );
  // For remote projects, derive workstation name from any branch (they all share the same workspace)
  let projectWorkstationName = $derived<string | null>(
    project.location === 'remote'
      ? (branches.find((b) => b.workspaceName)?.workspaceName ?? null)
      : null
  );

  let addRepoModalOpen = $state(false);

  function statusLabel(status: WorkspaceStatus | null): string {
    switch (status) {
      case 'starting':
        return 'Provisioning';
      case 'running':
        return 'Running';
      case 'stopped':
        return 'Stopped';
      case 'suspended':
        return 'Suspended';
      case 'error':
        return 'Error';
      default:
        return '';
    }
  }

  function handleRepoSelected(selection: RepoPickerSelection) {
    addRepoModalOpen = false;
    onRepoSelected?.(selection);
  }

  function repoForBranch(branch: Branch): ProjectRepo | null {
    if (!branch.projectRepoId) return null;
    return reposById.get(branch.projectRepoId) ?? null;
  }

  // ── Project session input ──────────────────────────────────────────────
  let promptText = $state('');
  let promptTextarea = $state<HTMLElement | null>(null);
  let availableAgents = $derived(agentState.providers);
  let preferredProvider = $derived(
    getPreferredAgentForProject(project.id, availableAgents) ?? undefined
  );
  let canSubmitPrompt = $derived(!!promptText.trim() && !!preferredProvider);
  let sendButtonTitle = $derived(
    preferredProvider ? 'Start project session' : 'No AI agent available'
  );
  /** Session IDs for running project sessions (all produce notes). */
  let activeSessionIds = $state<Set<string>>(new Set());

  // ── Live session hints (show latest agent message for running notes) ──
  let liveSessionHints = $state<Record<string, string>>({});
  const liveSessionHintPoller = createLiveSessionHints(
    (nextHints) => {
      liveSessionHints = nextHints;
    },
    () => branches.find((b) => b.worktreePath)?.worktreePath ?? null
  );

  /** Collect session IDs from running project notes + activeSessionIds. */
  let runningNoteSessionIds = $derived.by(() => {
    const ids = new Set<string>();
    for (const note of projectNotes) {
      if (!note.title.trim() && !note.content.trim() && note.sessionId) {
        ids.add(note.sessionId);
      }
    }
    for (const sid of activeSessionIds) {
      ids.add(sid);
    }
    return Array.from(ids);
  });

  $effect(() => {
    liveSessionHintPoller.syncRunningSessionIds(runningNoteSessionIds);
  });

  onDestroy(() => {
    liveSessionHintPoller.destroy();
  });

  // Hashtag reference items
  let hashtagItems = $state<HashtagItem[]>([]);
  $effect(() => {
    buildProjectHashtagItems(project.id, branches, reposById).then((items) => {
      hashtagItems = items;
    });
  });

  // Image attachment state
  let imageIds = $state<string[]>([]);
  let imagePreviews = $state<Map<string, string>>(new Map());
  let imageFileInput = $state<HTMLInputElement>();
  let dragOver = $state(false);
  let promptWrapperEl: HTMLDivElement | undefined = $state();

  // Load previews for attached images
  $effect(() => {
    for (const id of imageIds) {
      if (!imagePreviews.has(id)) {
        getImageData(id)
          .then((dataUrl) => {
            imagePreviews = new Map(imagePreviews);
            imagePreviews.set(id, dataUrl);
          })
          .catch(() => {
            // Image may have been deleted — insert sentinel to prevent infinite retry
            imagePreviews = new Map(imagePreviews);
            imagePreviews.set(id, '');
          });
      }
    }
  });

  function openImagePicker() {
    imageFileInput?.click();
  }

  async function handleImageFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;
    for (const file of Array.from(input.files)) {
      await addImageFile(file);
    }
    input.value = '';
  }

  async function addImageFile(file: File) {
    const buffer = await file.arrayBuffer();
    const bytes = new Uint8Array(buffer);
    const chunks: string[] = [];
    for (let i = 0; i < bytes.length; i += 8192) {
      chunks.push(String.fromCharCode(...bytes.subarray(i, i + 8192)));
    }
    const base64 = btoa(chunks.join(''));
    try {
      const image = await createImageFromData(null, project.id, file.name, file.type, base64, true);
      imageIds = [...imageIds, image.id];
      const dataUrl = `data:${file.type};base64,${base64}`;
      imagePreviews = new Map(imagePreviews);
      imagePreviews.set(image.id, dataUrl);
    } catch (err) {
      console.error('Failed to attach image:', err);
    }
  }

  function handleImagePaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const item of Array.from(items)) {
      if (item.type.startsWith('image/')) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) void addImageFile(file);
      }
    }
  }

  function removeImage(imageId: string) {
    imageIds = imageIds.filter((id) => id !== imageId);
    imagePreviews = new Map(imagePreviews);
    imagePreviews.delete(imageId);
    deleteImage(imageId).catch((err) => {
      console.error('Failed to delete image:', err);
    });
  }

  async function handleFileDrop(paths: string[]) {
    const imagePaths = paths.filter((p) => isImageFile(p));
    const pid = project.id;
    const newIds: string[] = [];
    for (const path of imagePaths) {
      try {
        const image = await createImage(null, pid, path, true);
        newIds.push(image.id);
      } catch (e) {
        console.error('Failed to create image from dropped file:', e);
      }
    }
    if (newIds.length > 0) {
      imageIds = [...imageIds, ...newIds];
    }
  }

  // Subscribe to drag-drop service
  $effect(() => {
    const el = promptWrapperEl;
    if (!el) return;
    const unsub = untrack(() =>
      subscribeDragDrop({
        element: el,
        onDragOver: (over) => {
          dragOver = over;
        },
        onDrop: (paths) => {
          handleFileDrop(paths);
        },
      })
    );
    return unsub;
  });

  function autoResize(el: HTMLElement) {
    el.style.height = 'auto';
    el.style.height = `${el.scrollHeight}px`;
  }

  async function handleSubmitPrompt() {
    const text = promptText.trim();
    const provider = preferredProvider;
    if (!text || !provider) return;

    const imageIdsToSend = imageIds.length > 0 ? [...imageIds] : undefined;
    promptText = '';
    imageIds = [];
    imagePreviews = new Map();
    if (promptTextarea) {
      promptTextarea.style.height = 'auto';
    }
    try {
      const response = await commands.startProjectSession(
        project.id,
        text,
        provider,
        imageIdsToSend
      );
      activeSessionIds = new Set([...activeSessionIds, response.sessionId]);
      sessionRegistry.register(response.sessionId, project.id, 'note');
      projectStateStore.addRunningSession(project.id, response.sessionId);
      // Reload notes immediately so the stub appears as "Generating note…"
      await loadProjectNotes();
    } catch (e) {
      console.error('[ProjectSection] Failed to start project session:', e);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.altKey) {
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

  /** All notes: completed (oldest first) followed by generating – matches branch timeline order. */
  let timelineNotes = $derived(
    [...projectNotes].sort((a, b) => {
      const aIsGenerating = !a.title.trim() && !a.content.trim();
      const bIsGenerating = !b.title.trim() && !b.content.trim();
      if (aIsGenerating !== bIsGenerating) return aIsGenerating ? 1 : -1;
      return (a.completedAt ?? a.createdAt) - (b.completedAt ?? b.createdAt);
    })
  );

  let openNote = $state<{ title: string; content: string; sessionId?: string } | null>(null);
  let openSessionId = $state<string | null>(null);

  // ── Lifecycle ──────────────────────────────────────────────────────────

  onMount(() => {
    loadProjectNotes();

    let unlistenSession: (() => void) | undefined;
    listen<{ sessionId: string; status: string; projectId?: string }>(
      'session-status-changed',
      (event) => {
        const { sessionId, status, projectId } = event.payload;
        const isTracked = activeSessionIds.has(sessionId);
        // Also reload if this session belongs to a known project note (handles
        // sessions that were already running when the component mounted).
        const isKnownNoteSession = projectNotes.some((n) => n.sessionId === sessionId);

        // Handle resumed sessions: when a project note session is resumed,
        // the backend emits a "running" event with the projectId. Re-add it
        // to active tracking so the row spinner and sidebar spinner appear.
        if (
          status === 'running' &&
          !isTracked &&
          isKnownNoteSession &&
          (projectId === project.id || !projectId)
        ) {
          activeSessionIds = new Set([...activeSessionIds, sessionId]);
          sessionRegistry.register(sessionId, project.id, 'note');
          projectStateStore.addRunningSession(project.id, sessionId);
        }

        if (isTracked || isKnownNoteSession) {
          if (status === 'completed' || status === 'error' || status === 'cancelled') {
            if (isTracked) {
              const next = new Set(activeSessionIds);
              next.delete(sessionId);
              activeSessionIds = next;
            }
            // Refresh notes after session completes
            loadProjectNotes();
          }
        }
      }
    ).then((unlisten) => {
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
      {#if projectWorkspaceStatus}
        <div
          class="workspace-status-badge"
          class:starting={projectWorkspaceStatus === 'starting'}
          class:running={projectWorkspaceStatus === 'running'}
          class:stopped={projectWorkspaceStatus === 'stopped'}
          class:suspended={projectWorkspaceStatus === 'suspended'}
          class:error={projectWorkspaceStatus === 'error'}
          title={projectWorkspaceStatus === 'running' && projectWorkstationName
            ? projectWorkstationName
            : undefined}
        >
          {#if projectWorkspaceStatus === 'starting'}
            <Spinner size={12} />
          {:else if projectWorkspaceStatus === 'running'}
            <Cloud size={12} />
          {:else if projectWorkspaceStatus === 'stopped'}
            <CirclePause size={12} />
          {:else if projectWorkspaceStatus === 'suspended'}
            <Pause size={12} />
          {:else if projectWorkspaceStatus === 'error'}
            <AlertCircle size={12} />
          {/if}
          <span>{statusLabel(projectWorkspaceStatus)}</span>
          {#if projectWorkspaceStatus === 'suspended' && projectWorkstationName}
            <button
              class="resume-button"
              onclick={() => onResumeWorkspace?.(projectWorkstationName!)}
              title="Resume suspended workspace"
            >
              Resume
            </button>
          {/if}
        </div>
      {/if}
    </div>
    {#if !deleting}
      <div class="header-actions">
        <button
          class="header-action-button"
          onclick={() => {
            addRepoModalOpen = true;
          }}
          disabled={addRepoDisabled}
          title={addRepoTitle}
        >
          <span class="action-icon"><Plus size={12} /></span>
          Add Repo
        </button>
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
    <input
      bind:this={imageFileInput}
      type="file"
      accept="image/png,image/jpeg,image/gif,image/webp,image/svg+xml"
      multiple
      class="file-input-hidden"
      onchange={handleImageFileSelect}
    />
    <div class="prompt-input-wrapper" class:drag-over={dragOver} bind:this={promptWrapperEl}>
      <div class="prompt-input-row">
        {#if imageIds.length === 0}
          <button class="attach-btn" onclick={openImagePicker} title="Attach image">
            <Paperclip size={14} />
          </button>
        {/if}
        <HashtagInput
          class="prompt-input"
          placeholder="Ask about this project…"
          bind:value={promptText}
          bind:textareaEl={promptTextarea}
          onkeydown={handleKeydown}
          oninput={(e) => autoResize(e.currentTarget as HTMLElement)}
          onpaste={handleImagePaste}
          rows={1}
          items={hashtagItems}
        />
        <div class="prompt-actions">
          <AgentSelector projectId={project.id} />
          <button
            class="send-button"
            onclick={handleSubmitPrompt}
            disabled={!canSubmitPrompt}
            title={sendButtonTitle}
          >
            <Send size={14} />
          </button>
        </div>
      </div>
      {#if imageIds.length > 0}
        <div class="reply-images">
          {#each imageIds as imageId}
            <div class="reply-image-thumb">
              {#if imagePreviews.get(imageId)}
                <img src={imagePreviews.get(imageId)} alt="attached" />
              {:else}
                <div class="reply-image-placeholder"><ImagePlus size={16} /></div>
              {/if}
              <button
                class="reply-image-remove"
                onclick={() => removeImage(imageId)}
                title="Remove image"
              >
                <X size={10} />
              </button>
            </div>
          {/each}
          <button class="reply-image-add" onclick={openImagePicker} title="Add image">
            <Plus size={16} />
          </button>
        </div>
      {/if}
    </div>
  </div>

  <!-- Project notes -->
  {#if projectNotes.length > 0}
    {@const nowMs = minuteNow.now()}
    <div class="project-notes">
      <div class="notes-header">
        <FileText size={13} />
        <span>Project Notes</span>
      </div>
      <div class="notes-timeline">
        {#each timelineNotes as note, index (note.id)}
          {@const isRunning = !note.title.trim() && !note.content.trim()}
          {@const isFailed = !isRunning && !!note.sessionId && !note.content.trim()}
          {@const noteType = isRunning ? 'generating-note' : isFailed ? 'failed-note' : 'note'}
          {@const liveHint =
            isRunning && note.sessionId ? liveSessionHints[note.sessionId] : undefined}
          <TimelineRow
            type={noteType}
            title={isRunning
              ? 'Generating note…'
              : isFailed
                ? 'Session finished — no note created'
                : note.title || 'Untitled note'}
            secondaryMeta={isRunning
              ? (liveHint ?? 'Generating note')
              : isFailed
                ? undefined
                : formatRelativeTime(note.completedAt ?? note.createdAt, nowMs)}
            deleting={deletingNoteIds.has(note.id)}
            isLast={index === timelineNotes.length - 1}
            sessionId={note.sessionId ?? undefined}
            onItemClick={isRunning || isFailed
              ? undefined
              : () => {
                  openNote = {
                    title: note.title,
                    content: note.content,
                    sessionId: note.sessionId ?? undefined,
                  };
                }}
            onSessionClick={(sid) => {
              openSessionId = sid;
            }}
            onDeleteClick={() => handleDeleteNote(note.id)}
          />
        {/each}
      </div>
    </div>
  {/if}

  <div class="branches-list" class:deleting>
    {#each sortedBranches as branch (branch.id)}
      <BranchCard
        {branch}
        repoLabel={repoForBranch(branch)}
        projectName={project.name}
        deleting={deletingBranches.has(branch.id)}
        worktreeError={worktreeErrors.get(branch.id)}
        workspaceError={workspaceErrors.get(branch.id)}
        onDelete={() => onDeleteBranch?.(branch.id)}
        onRename={(branchName) => onRenameBranch?.(branch.id, branchName)}
        onRetryWorktree={() => onRetryWorktree?.(branch.id)}
        {onDismissReason}
      />
    {/each}
  </div>
</div>

{#if openNote}
  <NoteModal
    title={openNote.title}
    content={openNote.content}
    sessionId={openNote.sessionId}
    onClose={() => (openNote = null)}
    onOpenSession={(sid) => {
      openNote = null;
      openSessionId = sid;
    }}
  />
{/if}

{#if openSessionId}
  {@const noteForSession = projectNotes.find(
    (n) => n.sessionId === openSessionId && n.content?.trim()
  )}
  <SessionModal
    sessionId={openSessionId}
    repoDir={branches.find((b) => b.worktreePath)?.worktreePath ?? null}
    projectId={project.id}
    noteInfo={noteForSession
      ? { id: noteForSession.id, title: noteForSession.title, content: noteForSession.content }
      : null}
    onOpenNote={(_noteId, title, content) => {
      const sid = openSessionId;
      openSessionId = null;
      openNote = { title, content, sessionId: sid ?? undefined };
    }}
    onClose={() => {
      openSessionId = null;
      loadProjectNotes();
    }}
  />
{/if}

{#if addRepoModalOpen}
  <AddRepoModal
    {excludeRepos}
    onAdded={handleRepoSelected}
    onClose={() => {
      addRepoModalOpen = false;
    }}
  />
{/if}

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
    padding: 16px 0 8px;
    position: sticky;
    top: 0;
    z-index: 10;
    background-color: var(--bg-chrome);
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

  .workspace-status-badge {
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

  .workspace-status-badge.starting {
    border-color: var(--ui-info);
    color: var(--ui-info);
  }

  .workspace-status-badge.running {
    border-color: var(--border-muted);
    color: var(--text-primary);
  }

  .workspace-status-badge.stopped {
    border-color: var(--border-muted);
    color: var(--text-muted);
  }

  .workspace-status-badge.suspended {
    border-color: var(--border-muted);
    color: var(--text-muted);
  }

  .resume-button {
    all: unset;
    cursor: pointer;
    margin-left: 4px;
    padding: 0 4px;
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
    color: var(--ui-info);
    border-left: 1px solid var(--border-muted);
  }

  .resume-button:hover {
    text-decoration: underline;
  }

  .workspace-status-badge.error {
    border-color: var(--ui-danger);
    color: var(--ui-danger);
  }

  /* ── Project prompt ──────────────────────────────────────────────────── */

  .project-prompt-section {
    padding: 0;
  }

  .file-input-hidden {
    display: none;
  }

  .prompt-input-wrapper {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background-color: var(--bg-primary);
    transition:
      border-color 0.15s ease,
      background-color 0.15s ease;
  }

  .prompt-input-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 6px 8px;
  }

  .prompt-input-wrapper:focus-within {
    border-color: var(--border-emphasis);
  }

  .prompt-input-wrapper.drag-over {
    border-color: var(--ui-accent);
    background-color: color-mix(in srgb, var(--ui-accent) 6%, var(--bg-primary));
  }

  .attach-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text-faint);
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color 0.15s ease,
      background-color 0.15s ease;
  }

  .attach-btn:hover {
    color: var(--text-primary);
    background-color: var(--ui-selection);
  }

  .prompt-input-row :global(.prompt-input) {
    flex: 1;
    margin: 0;
    padding: 4px 0;
    border: none;
    background: none;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    line-height: 1.5;
    resize: none;
    outline: none;
    min-height: 28px;
    max-height: 120px;
    overflow-y: auto;
  }

  .prompt-input-row :global(.hashtag-input-wrapper) {
    flex: 1;
  }

  .prompt-actions {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
  }

  .send-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: 8px;
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

  /* ── Reply image previews ─────────────────────────────────────────── */

  .reply-images {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    padding: 0 8px 8px;
  }

  .reply-image-thumb {
    position: relative;
    width: 48px;
    height: 48px;
    border-radius: 6px;
    overflow: hidden;
    border: 1px solid var(--border-muted);
    background: var(--bg-hover);
    flex-shrink: 0;
  }

  .reply-image-thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .reply-image-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    color: var(--text-faint);
  }

  .reply-image-remove {
    position: absolute;
    top: 2px;
    right: 2px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--bg-deepest);
    color: var(--text-muted);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      color 0.1s;
  }

  .reply-image-thumb:hover .reply-image-remove {
    opacity: 1;
  }

  .reply-image-remove:hover {
    color: var(--text-primary);
    background: var(--bg-chrome);
  }

  .reply-image-add {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    border-radius: 6px;
    border: 1px dashed var(--border-muted);
    background: none;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .reply-image-add:hover {
    color: var(--text-muted);
    border-color: var(--border-emphasis);
  }

  /* ── Project notes ───────────────────────────────────────────────────── */

  .project-notes {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 0;
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

  .notes-timeline {
    display: flex;
    flex-direction: column;
    padding: 0 8px;
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

  @media (max-width: 720px) {
    .prompt-input-row {
      padding: 6px;
    }

    .prompt-input-row :global(.prompt-input) {
      padding: 4px 2px;
      font-size: var(--size-xs);
    }

    .prompt-actions {
      gap: 2px;
    }

    .send-button {
      width: 30px;
      height: 30px;
    }
  }
</style>
