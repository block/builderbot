<!--
  ProjectSection.svelte - Project overview card + branch cards

  Shows the project name, project notes, project-session entry point, repo
  controls, and all branch cards for this project.
-->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listenToEvent } from '../../transport';
  import FileText from '@lucide/svelte/icons/file-text';
  import type {
    AcpConfigSelection,
    Project,
    ProjectRepo,
    Branch,
    ProjectNote,
    HashtagItem,
  } from '../../types';
  import * as commands from '../../api/commands';
  import { buildProjectHashtagItems } from '../sessions/hashtagItems';
  import { branchTimelineReadyKey } from '../branches/branchTimelineReady';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import BranchCard from '../branches/BranchCard.svelte';
  import { isSessionActive } from '../../shared/sessionStatus';
  import { deleteSessionLinkedItem } from '../../shared/deleteSessionLinkedItem';
  import SuggestedRepos from './SuggestedRepos.svelte';
  import type { RepoSelection as RepoPickerSelection } from '../../shared/githubUrl';
  import TimelineRow from '../timeline/TimelineRow.svelte';
  import TimelineContextMenu, {
    type TimelineContextMenuAction,
  } from '../timeline/TimelineContextMenu.svelte';
  import NoteModal from '../notes/NoteModal.svelte';
  import NewSessionModal from '../sessions/NewSessionModal.svelte';
  import { agentState } from '../agents/agent.svelte';
  import { getPreferredAgent } from '../settings/preferences.svelte';
  import { formatRelativeTime, minuteNow } from '../../shared/relativeTime.svelte';
  import { buildReferringPrompt } from '../../shared/buildReferringPrompt';
  import { createLiveSessionHints } from '../timeline/liveSessionHints';
  import { projectDisplayName } from '../../shared/utils';
  import { Button } from '$lib/components/ui/button';
  import { openDiffRoute } from '../layout/navigation.svelte';
  import {
    disabledReferenceNav,
    pushReferenceEntry,
    resolveHashtagReference,
    type HashtagClickInfo,
    type ReferenceDiffContext,
    type ReferenceHistoryEntry,
  } from '../references/referenceHistory.svelte';

  interface Props {
    project: Project;
    branches: Branch[];
    reposById?: Map<string, ProjectRepo>;
    deleting?: boolean;
    deletingBranches?: Set<string>;
    worktreeErrors?: Map<string, string>;
    workspaceErrors?: Map<string, string>;
    onDeleteBranch?: (branchId: string) => void;
    onRenameBranch?: (branchId: string, branchName: string) => void | Promise<void>;
    onProjectTitleElement?: (element: HTMLHeadingElement | null) => void;
    onRepoSelected?: (selection: RepoPickerSelection) => void | Promise<void>;
    onRetryWorktree?: (branchId: string) => void;
  }

  let {
    project,
    branches,
    reposById = new Map(),
    deleting = false,
    deletingBranches = new Set(),
    worktreeErrors = new Map(),
    workspaceErrors = new Map(),
    onDeleteBranch,
    onRenameBranch,
    onProjectTitleElement,
    onRepoSelected,
    onRetryWorktree,
  }: Props = $props();

  let sortedBranches = $derived([...branches].sort((a, b) => b.createdAt - a.createdAt));
  let projectDisplayRootCandidates = $derived(
    branches.map((branch) => branch.worktreePath).filter((path): path is string => !!path)
  );
  function repoForBranch(branch: Branch): ProjectRepo | null {
    if (!branch.projectRepoId) return null;
    return reposById.get(branch.projectRepoId) ?? null;
  }

  function reportProjectTitleElement(node: HTMLHeadingElement) {
    onProjectTitleElement?.(node);

    return {
      destroy() {
        onProjectTitleElement?.(null);
      },
    };
  }

  // ── Project session dialog ─────────────────────────────────────────────
  let showProjectSessionModal = $state(false);
  let draftProjectPrompt = $state('');
  let draftProjectImageIds = $state<string[]>([]);
  let preferredProvider = $derived(getPreferredAgent(agentState.providers) ?? undefined);
  /** Session IDs for running project sessions (all produce notes). */
  let activeSessionIds = $state<Set<string>>(new Set());

  // ── Live session hints/titles (latest agent message + ACP title for running notes) ──
  let liveSessionHints = $state<Record<string, string>>({});
  let liveSessionTitles = $state<Record<string, string>>({});
  const liveSessionHintPoller = createLiveSessionHints(
    (nextHints) => {
      liveSessionHints = nextHints;
    },
    () => projectDisplayRootCandidates,
    (nextTitles) => {
      liveSessionTitles = nextTitles;
    }
  );

  /** Collect session IDs from running project notes + activeSessionIds. */
  let runningNoteSessionIds = $derived.by(() => {
    const ids = new Set<string>();
    for (const note of projectNotes) {
      if (isSessionActive(note.sessionStatus) && note.sessionId) {
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
  let hashtagVersion = $state(0);
  let hashtagLoadGeneration = 0;
  let loadedHashtagSignature: string | null = null;
  let loadingHashtagSignature: string | null = null;
  let hashtagSignature = $derived.by(() => {
    const readyBranchParts = branches.map((branch) => {
      const readyKey = branchTimelineReadyKey(branch) ?? '';
      const repo = branch.projectRepoId ? reposById.get(branch.projectRepoId) : null;
      return [
        branch.id,
        readyKey,
        branch.branchName,
        branch.projectRepoId ?? '',
        repo?.githubRepo ?? '',
        repo?.subpath ?? '',
      ].join(':');
    });
    const noteParts = projectNotes.map((note) =>
      [note.id, note.title, note.updatedAt, note.completedAt ?? ''].join(':')
    );

    return [project.id, hashtagVersion, readyBranchParts.join('|'), noteParts.join('|')].join(';');
  });
  let referenceDiffContext = $derived<ReferenceDiffContext>({
    projectId: project.id,
    projectName: project.name,
  });

  async function ensureHashtagItems() {
    const signature = hashtagSignature;
    if (signature === loadedHashtagSignature) return;
    if (signature === loadingHashtagSignature) return;

    const generation = ++hashtagLoadGeneration;
    loadingHashtagSignature = signature;
    const branchesSnapshot = [...branches];
    const reposSnapshot = new Map(reposById);
    const notesSnapshot = [...projectNotes];

    try {
      const items = await buildProjectHashtagItems(
        project.id,
        branchesSnapshot,
        reposSnapshot,
        notesSnapshot
      );
      if (generation !== hashtagLoadGeneration) return;
      if (signature !== hashtagSignature) return;
      hashtagItems = items;
      loadedHashtagSignature = signature;
    } catch (err) {
      console.error('[ProjectSection] Failed to build hashtag items:', err);
    } finally {
      if (loadingHashtagSignature === signature) {
        loadingHashtagSignature = null;
      }
    }
  }

  $effect(() => {
    const _signature = hashtagSignature;
    if (!showProjectSessionModal) return;
    void ensureHashtagItems();
  });

  function openProjectSessionModal() {
    showProjectSessionModal = true;
    void ensureHashtagItems();
  }

  async function handleSubmitProjectSession(data: {
    prompt: string;
    imageIds: string[];
    provider?: string;
    acpConfigSelection?: AcpConfigSelection | null;
  }) {
    const text = data.prompt.trim();
    const provider = data.provider ?? preferredProvider;
    if (!text || !provider) return;

    const imageIdsToSend = data.imageIds.length > 0 ? [...data.imageIds] : undefined;
    try {
      const response = await commands.startProjectSession(
        project.id,
        text,
        provider,
        imageIdsToSend,
        data.acpConfigSelection ?? undefined
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
    const note = projectNotes.find((n) => n.id === noteId);
    const sessionId = note?.sessionId ?? undefined;
    deletingNoteIds = new Set([...deletingNoteIds, noteId]);
    try {
      await deleteSessionLinkedItem(() => commands.deleteProjectNote(noteId), sessionId);
      projectNotes = projectNotes.filter((n) => n.id !== noteId);
      hashtagVersion++;
      window.dispatchEvent(new CustomEvent('project-notes-invalidated'));
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
      const aIsActive = isSessionActive(a.sessionStatus);
      const bIsActive = isSessionActive(b.sessionStatus);
      if (aIsActive !== bIsActive) return aIsActive ? 1 : -1;
      return (a.completedAt ?? a.createdAt) - (b.completedAt ?? b.createdAt);
    })
  );
  let projectNoteContextMenuActions = $derived.by(() => {
    const actions: TimelineContextMenuAction[] = [];
    for (const note of timelineNotes) {
      const deleting = deletingNoteIds.has(note.id);
      actions.push({
        key: projectNoteContextMenuKey(note),
        hashtagRef: isCompletedProjectNote(note) ? `#project-note:${note.id}` : undefined,
        onDelete: deleting ? undefined : () => handleDeleteNote(note.id),
      });
    }
    return actions;
  });

  type OpenProjectNoteState = {
    noteId: string;
    title: string;
    content: string;
    sessionId?: string;
    noteUpdatedAt?: number;
    chatOpen?: boolean;
  };

  let openNote = $state<OpenProjectNoteState | null>(null);

  $effect(() => {
    const _signature = hashtagSignature;
    if (!openNote) return;
    void ensureHashtagItems();
  });

  function isCompletedProjectNote(note: ProjectNote): boolean {
    const isRunning = isSessionActive(note.sessionStatus);
    const isFailed = !isRunning && !!note.sessionId && !note.content.trim();
    return !isRunning && !isFailed;
  }

  function projectNoteContextMenuKey(note: ProjectNote): string {
    return `project-note-${note.id}`;
  }

  function handleProjectNoteNewSessionReferring(ref: string) {
    draftProjectPrompt = buildReferringPrompt(draftProjectPrompt, ref);
    openProjectSessionModal();
  }

  function projectNoteToOpenState(note: ProjectNote, chatOpen = false): OpenProjectNoteState {
    return {
      noteId: note.id,
      title: note.title,
      content: note.content,
      sessionId: note.sessionId ?? undefined,
      noteUpdatedAt: note.updatedAt,
      chatOpen,
    };
  }

  function openProjectNote(note: ProjectNote, chatOpen = false) {
    openNote = projectNoteToOpenState(note, chatOpen);
  }

  function currentDialogReferenceEntry(): ReferenceHistoryEntry | null {
    if (openNote) {
      return {
        kind: 'note',
        noteKind: 'project',
        id: openNote.noteId,
        ref: `#project-note:${openNote.noteId}`,
        title: openNote.title,
        content: openNote.content,
        view: openNote.chatOpen ? 'chat' : 'note',
        sessionId: openNote.sessionId,
        noteUpdatedAt: openNote.noteUpdatedAt,
        projectId: project.id,
        repoDir: projectDisplayRootCandidates,
        hashtagItems,
        diffContext: referenceDiffContext,
      };
    }

    return null;
  }

  function closeReferenceDialogs() {
    openNote = null;
  }

  function handleHashtagClick(click: HashtagClickInfo) {
    const target = resolveHashtagReference(click, {
      hashtagItems,
      diffContext: referenceDiffContext,
    });
    if (!target) return;

    const current = currentDialogReferenceEntry();
    if (current) pushReferenceEntry(current);
    pushReferenceEntry(target);
    closeReferenceDialogs();
    if (target.kind === 'diff') {
      openDiffRoute(target.route);
    }
  }

  function handleOpenInnerSession(sessionId: string) {
    const current = currentDialogReferenceEntry();
    if (current) pushReferenceEntry(current);
    pushReferenceEntry({
      kind: 'chat',
      ref: `#chat:${sessionId}`,
      sessionId,
      projectId: project.id,
      repoDir: projectDisplayRootCandidates,
      hashtagItems,
      diffContext: referenceDiffContext,
    });
    closeReferenceDialogs();
  }

  // ── Lifecycle ──────────────────────────────────────────────────────────

  onMount(() => {
    loadProjectNotes();

    // Refresh hashtag items when branch timelines are invalidated (e.g. branch session completion)
    const onTimelineInvalidated = () => {
      hashtagVersion++;
    };
    window.addEventListener('timeline-invalidated', onTimelineInvalidated);

    const unlistenSession = listenToEvent<{
      sessionId: string;
      status: string;
      projectId?: string;
    }>('session-status-changed', async (payload) => {
      const { sessionId, status, projectId } = payload;
      if (projectId !== project.id) return;

      if (status === 'running') {
        // Bridge: track until the stub (already loaded by startProjectSession) is
        // updated with an authoritative sessionStatus on the terminal event.
        activeSessionIds = new Set([...activeSessionIds, sessionId]);
        return;
      }

      if (status === 'completed' || status === 'error' || status === 'cancelled') {
        const next = new Set(activeSessionIds);
        next.delete(sessionId);
        activeSessionIds = next;

        // Surgically update just the affected note instead of reloading all
        const updatedNote = await commands.getProjectNoteBySession(sessionId);
        if (updatedNote) {
          projectNotes = projectNotes.map((n) => (n.id === updatedNote.id ? updatedNote : n));
        } else {
          // Note was filtered out (e.g. deleted) — remove from local list
          projectNotes = projectNotes.filter((n) => n.sessionId !== sessionId);
        }

        // Invalidate timeline caches so hashtag items pick up new commits/notes
        for (const b of branches) {
          commands.invalidateBranchTimeline(b.id);
        }
        hashtagVersion++;
      }
    });

    return () => {
      unlistenSession();
      window.removeEventListener('timeline-invalidated', onTimelineInvalidated);
    };
  });
</script>

<div class="project-section">
  <section class="project-overview-card">
    <h2 class="project-title" use:reportProjectTitleElement>{projectDisplayName(project)}</h2>

    {#if projectNotes.length > 0}
      {@const nowMs = minuteNow.now()}
      <TimelineContextMenu
        actions={projectNoteContextMenuActions}
        onNewSessionReferring={handleProjectNoteNewSessionReferring}
      >
        <div class="notes-timeline">
          {#each timelineNotes as note, index (note.id)}
            {@const isRunning = isSessionActive(note.sessionStatus)}
            {@const isFailed = !isRunning && !!note.sessionId && !note.content.trim()}
            {@const noteType = isRunning ? 'generating-note' : isFailed ? 'failed-note' : 'note'}
            {@const liveHint =
              isRunning && note.sessionId ? liveSessionHints[note.sessionId] : undefined}
            {@const liveTitle =
              isRunning && note.sessionId ? liveSessionTitles[note.sessionId] : undefined}
            <TimelineRow
              type={noteType}
              title={isRunning
                ? (liveTitle ?? 'Generating note…')
                : isFailed
                  ? 'Session finished — no note created'
                  : note.title || 'Untitled note'}
              secondaryMeta={isRunning
                ? (liveHint ?? 'Generating note')
                : isFailed
                  ? undefined
                  : formatRelativeTime(note.completedAt ?? note.createdAt, nowMs)}
              deleting={deletingNoteIds.has(note.id)}
              isLast={false}
              sessionId={note.sessionId ?? undefined}
              onItemClick={isRunning || isFailed ? undefined : () => openProjectNote(note)}
              onSessionClick={(sid) => {
                openProjectNote({ ...note, sessionId: sid }, true);
              }}
              deleteDisabledReason={deletingNoteIds.has(note.id) ? 'Deleting...' : undefined}
              onDeleteClick={() => handleDeleteNote(note.id)}
              contextMenuKey={projectNoteContextMenuKey(note)}
            />
          {/each}
        </div>
      </TimelineContextMenu>
    {/if}

    <div class="project-session-footer" class:empty={projectNotes.length === 0}>
      <span class={projectNotes.length === 0 ? 'inline-flex flex-1' : 'inline-flex'}>
        <Button
          variant="ghost"
          onclick={openProjectSessionModal}
          aria-label="New project note"
          class={[
            'inline-flex items-center font-medium transition-[color,background-color,border-color,box-shadow,opacity] duration-300',
            '[&_svg]:transition-colors [&_svg]:duration-300',
            projectNotes.length === 0
              ? 'flex-1 justify-center gap-2 px-1.5 py-2.5 h-auto rounded-lg border border-solid border-transparent bg-[var(--bg-elevated)] text-sm hover:bg-[var(--note-bg)] hover:text-[var(--note-color)] [&_svg]:!size-[18px] [&_svg]:text-[var(--note-color)]'
              : 'gap-[5px] px-2.5 h-8 rounded-md border border-dashed border-[var(--border-subtle)] bg-transparent text-xs hover:border-[var(--note-color)] hover:bg-[var(--note-bg)] hover:text-[var(--note-color)] [&_svg]:!size-[13px] [&_svg]:text-[var(--note-color)]',
          ]}
        >
          <FileText size={18} />
          <span>New project note</span>
        </Button>
      </span>
    </div>
  </section>

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
      />
    {/each}
  </div>

  <SuggestedRepos {project} {reposById} {onRepoSelected} />
</div>

{#if showProjectSessionModal}
  <NewSessionModal
    open={true}
    {project}
    mode="note"
    initialPrompt={draftProjectPrompt}
    initialImageIds={draftProjectImageIds}
    {hashtagItems}
    submitDisabledReason={preferredProvider ? null : 'No AI agent available'}
    onClose={(draft) => {
      draftProjectPrompt = draft.prompt;
      draftProjectImageIds = draft.imageIds;
      showProjectSessionModal = false;
    }}
    onSubmit={(data) => {
      draftProjectPrompt = '';
      draftProjectImageIds = [];
      void handleSubmitProjectSession({
        prompt: data.prompt,
        imageIds: data.imageIds,
        provider: data.provider,
        acpConfigSelection: data.acpConfigSelection,
      });
    }}
  />
{/if}

{#if openNote}
  <NoteModal
    open={true}
    title={openNote.title}
    content={openNote.content}
    sessionId={openNote.sessionId}
    noteUpdatedAt={openNote.noteUpdatedAt}
    noteId={openNote.noteId}
    noteKind="project"
    projectId={project.id}
    repoDir={projectDisplayRootCandidates}
    chatOpen={openNote.chatOpen ?? false}
    onChatOpenChange={(chatOpen) => {
      if (openNote) openNote = { ...openNote, chatOpen };
    }}
    {hashtagItems}
    referenceNav={disabledReferenceNav}
    onClose={() => {
      openNote = null;
      void loadProjectNotes();
    }}
    onOpenSession={handleOpenInnerSession}
    onHashtagClick={handleHashtagClick}
  />
{/if}

<style>
  .project-section {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .project-overview-card {
    --project-card-padding-inline: 16px;

    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px var(--project-card-padding-inline) 14px;
    border: 1px solid var(--border-subtle);
    border-radius: 8px;
    background-color: var(--bg-primary);
  }

  .project-title {
    min-width: 0;
    margin: 0;
    color: var(--text-primary);
    font-size: 22px;
    font-weight: 700;
    line-height: 1.2;
    overflow-wrap: anywhere;
  }

  .notes-timeline {
    --timeline-row-bleed: var(--project-card-padding-inline);

    display: flex;
    flex-direction: column;
  }

  .project-session-footer {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px 0;
    margin: 0 -8px;
  }

  .project-session-footer.empty {
    padding: 0;
    margin: 0;
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
    .project-section {
      gap: 14px;
    }

    .project-overview-card {
      --project-card-padding-inline: 14px;

      padding: 16px var(--project-card-padding-inline) 12px;
    }

    .project-title {
      font-size: 20px;
    }
  }
</style>
