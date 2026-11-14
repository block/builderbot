<!--
  ProjectSection.svelte - Project overview card + branch cards

  Shows the project name, project notes, project-session entry point, repo
  controls, and all branch cards for this project.
-->
<script lang="ts">
  import { onMount, onDestroy, tick, untrack } from 'svelte';
  import { toast } from 'svelte-sonner';
  import { listenToEvent } from '../../transport';
  import FileText from '@lucide/svelte/icons/file-text';
  import Pencil from '@lucide/svelte/icons/pencil';
  import type {
    AcpConfigSelection,
    ChildSessionInfo,
    Project,
    ProjectRepo,
    Branch,
    ProjectNote,
    HashtagItem,
    NoteTimelineItem,
  } from '../../types';
  import * as commands from '../../api/commands';
  import { buildProjectHashtagItems, projectNotesToHashtagItems } from '../sessions/hashtagItems';
  import { branchTimelineReadyKey } from '../branches/branchTimelineReady';
  import { sessionRegistry } from '../../stores/sessionRegistry.svelte';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import BranchCard from '../branches/BranchCard.svelte';
  import { isSessionActive } from '../../shared/sessionStatus';
  import { deleteSessionLinkedItem } from '../../shared/deleteSessionLinkedItem';
  import { LatestWins } from '../../shared/latestWins';
  import ConfirmDeleteDialog, {
    type ConfirmDeleteRequest,
  } from '../../shared/ConfirmDeleteDialog.svelte';
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
  import { minuteNow } from '../../shared/relativeTime.svelte';
  import { noteLastWrittenAt, noteReadState } from '../notes/noteTimestamps';
  import { forgetDeletedNote, setNoteReadState } from '../notes/noteReadActions';
  import { noteChatState } from '../sessions/noteFreshness';
  import { buildReferringPrompt } from '../../shared/buildReferringPrompt';
  import { createLiveSessionHints } from '../timeline/liveSessionHints';
  import { canResumeRow, noteRowPhase } from '../timeline/sessionRowState';
  import { resumeSessionFromRow } from '../timeline/resumeSessionFromRow';
  import { buildChildSessionBadges, runningChildSessionIds } from '../timeline/childSessionBadges';
  import { nextActiveSessionIds } from './activeSessionIds';
  import { projectNoteRow } from './projectNoteRow';
  import { projectDisplayName } from '../../shared/utils';
  import ProjectTagPills from './ProjectTagPills.svelte';
  import { Button } from '$lib/components/ui/button';
  import Spinner from '../../shared/Spinner.svelte';
  import {
    navigation,
    onLayerActivated,
    onViewHidden,
    openDiffRoute,
  } from '../layout/navigation.svelte';
  import { useWorkspaceLayer } from '../layout/workspaceLayer';
  import {
    disabledReferenceNav,
    pushReferenceEntry,
    resolveHashtagReference,
    type HashtagClickInfo,
    type ReferenceDiffContext,
    type ReferenceHistoryEntry,
  } from '../references/referenceHistory.svelte';
  import { createDraftWriteScheduler, loadDraft } from '../../shared/draftCache';
  import { replyDraftPresence } from '../sessions/replyDrafts';

  interface Props {
    project: Project;
    branches: Branch[];
    reposById?: Map<string, ProjectRepo>;
    deleting?: boolean;
    /** The project is being transferred: its branches and repos can't change
     *  until it's over, so their actions are dimmed like a delete's. */
    transferring?: boolean;
    deletingBranches?: Set<string>;
    worktreeErrors?: Map<string, string>;
    workspaceErrors?: Map<string, string>;
    onRenameProject?: (projectId: string, name: string) => void | Promise<Project | void>;
    onDeleteBranch?: (branchId: string) => void;
    onRenameBranch?: (branchId: string, branchName: string) => void | Promise<void>;
    onMoveBranch?: (branchId: string, targetProjectId: string) => void | Promise<void>;
    onProjectTitleElement?: (element: HTMLHeadingElement | null) => void;
    onRepoSelected?: (selection: RepoPickerSelection) => void | Promise<void>;
    onRetryWorktree?: (branchId: string) => void;
    /** Bumped to open the tag picker (the T shortcut, from ProjectHome). */
    tagPickerRequest?: number;
  }

  let {
    project,
    branches,
    reposById = new Map(),
    deleting = false,
    transferring = false,
    deletingBranches = new Set(),
    worktreeErrors = new Map(),
    workspaceErrors = new Map(),
    onRenameProject,
    onDeleteBranch,
    onRenameBranch,
    onMoveBranch,
    onProjectTitleElement,
    onRepoSelected,
    onRetryWorktree,
    tagPickerRequest = 0,
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

  // ── Inline project rename ──────────────────────────────────────────────
  let editingProjectName = $state(false);
  let projectNameDraft = $state('');
  let renameProjectError = $state<string | null>(null);
  let renameProjectSaving = $state(false);
  // The trimmed draft whose save last failed. A blur with the same draft
  // cancels instead of re-submitting, so the input losing focus after a
  // failure cannot loop through refocus → blur → identical rename → toast.
  let lastFailedProjectName = $state<string | null>(null);
  let projectNameInput = $state<HTMLInputElement | null>(null);

  async function focusProjectNameInput() {
    await tick();
    projectNameInput?.focus();
    projectNameInput?.select();
  }

  async function startProjectRename() {
    if (deleting || renameProjectSaving || !onRenameProject) return;
    projectNameDraft = project.name;
    renameProjectError = null;
    lastFailedProjectName = null;
    editingProjectName = true;
    await focusProjectNameInput();
  }

  function cancelProjectRename() {
    if (renameProjectSaving) return;
    editingProjectName = false;
    projectNameDraft = '';
    renameProjectError = null;
    lastFailedProjectName = null;
    dismissRenameProjectToast();
  }

  // The failure toast is pinned (infinite duration) so it cannot outlive the
  // error it describes: a successful retry or a cancel takes it down too.
  function dismissRenameProjectToast() {
    toast.dismiss(renameProjectToastId());
  }

  function renameProjectToastId() {
    return `rename-project-${project.id}`;
  }

  // A delete starting while the editor is open disables the input, which
  // blurs it. Close the editor instead of letting that blur rename a project
  // that is being torn down.
  $effect(() => {
    if (deleting) untrack(cancelProjectRename);
  });

  /**
   * Commit the draft. `source` decides what happens to a draft that cannot
   * be saved: Enter keeps the user in the input to fix it, while blur (the
   * user has moved on) cancels and restores the old name.
   */
  async function saveProjectRename(source: 'enter' | 'blur') {
    if (!editingProjectName || renameProjectSaving || deleting) return;

    const trimmed = projectNameDraft.trim();
    if (!trimmed) {
      if (source === 'blur') {
        cancelProjectRename();
        return;
      }
      renameProjectError = 'Project name is required';
      await focusProjectNameInput();
      return;
    }

    if (trimmed === project.name.trim()) {
      cancelProjectRename();
      return;
    }

    if (source === 'blur' && trimmed === lastFailedProjectName) {
      // The failure toast is still up; leaving the field means give up, not
      // retry the same name.
      cancelProjectRename();
      return;
    }

    if (!onRenameProject) return;
    renameProjectSaving = true;
    renameProjectError = null;
    try {
      await onRenameProject(project.id, trimmed);
      editingProjectName = false;
      projectNameDraft = '';
      lastFailedProjectName = null;
      dismissRenameProjectToast();
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e);
      renameProjectError = message;
      lastFailedProjectName = trimmed;
      toast.error('Unable to rename project', {
        // One toast per project: a retry replaces the previous failure
        // rather than stacking another infinite-duration toast.
        id: renameProjectToastId(),
        description: message,
        duration: Infinity,
      });
      // Clear the saving flag before focusing: the input is disabled while
      // saving, and focusProjectNameInput's tick() flushes that attribute
      // before it tries to focus.
      renameProjectSaving = false;
      await focusProjectNameInput();
      return;
    } finally {
      renameProjectSaving = false;
    }
  }

  function handleProjectNameKeydown(e: KeyboardEvent) {
    // An IME fires Enter to confirm a composition candidate; that is not a
    // submit, and acting on it would save the half-composed name.
    if (e.isComposing) return;
    if (e.key === 'Enter') {
      e.preventDefault();
      void saveProjectRename('enter');
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelProjectRename();
    }
  }

  // ── Project session dialog ─────────────────────────────────────────────
  let showProjectSessionModal = $state(false);
  let draftProjectPrompt = $state('');
  let draftProjectImageIds = $state<string[]>([]);
  const projectDraftWriter = createDraftWriteScheduler();
  let projectDraftLoadGeneration = 0;
  /** Bumped by every open so a referring open can tell when a plain open overtook it. */
  let projectSessionOpenRequest = 0;
  let preferredProvider = $derived(getPreferredAgent(agentState.providers) ?? undefined);
  /** Session IDs for running project sessions (all produce notes). */
  let activeSessionIds = $state<Set<string>>(new Set());

  function projectDraftKey(projectId: string): string {
    return `draft:project-session:${projectId}`;
  }

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

  // Running children ride along for their ACP title only: a child badge
  // reads `liveSessionTitles`, nothing reads a child's hint, so each child
  // costs one `getSession` per tick and never a transcript poll.
  $effect(() => {
    liveSessionHintPoller.syncRunningSessionIds(
      runningNoteSessionIds,
      runningChildSessionIds(childSessionsBySession)
    );
  });

  // This section stays mounted while home covers its project (and while the
  // workspace is parked under settings / the diff view); the hint poller's
  // get_session* round trips are for rows nobody can see, so pause it there
  // and resume — polling at once — on return.
  const layer = useWorkspaceLayer();
  $effect(() => {
    liveSessionHintPoller.setPaused(!layer.visible);
  });

  onDestroy(() => {
    liveSessionHintPoller.destroy();
    void projectDraftWriter.flush();
  });

  // ── In-flight repo sessions spawned by running project sessions ────────
  /** Queued and running children keyed by the project session that started them. */
  let childSessionsBySession = $state<Record<string, ChildSessionInfo[]>>({});
  let childSessionsGeneration = 0;

  /**
   * Rebuild the child map for the given project sessions.
   *
   * Rebuilt wholesale rather than patched, so a session that stopped running
   * drops out without a separate pruning pass.
   */
  async function refreshChildSessions(parentIds: string[]) {
    const generation = ++childSessionsGeneration;
    try {
      const entries = await Promise.all(
        parentIds.map(
          async (id) => [id, await commands.listChildSessions(id)] as [string, ChildSessionInfo[]]
        )
      );
      // A newer refresh already landed; its result is the current one.
      if (generation !== childSessionsGeneration) return;

      const next: Record<string, ChildSessionInfo[]> = {};
      for (const [id, children] of entries) {
        if (children.length > 0) next[id] = children;
      }
      childSessionsBySession = next;
    } catch (e) {
      console.error('[ProjectSection] Failed to load child sessions:', e);
    }
  }

  // The only trigger. Every `session-status-changed` branch below — and the
  // `timeline-invalidated` handler in onMount — reassigns `activeSessionIds`
  // to a *fresh* Set, so this derived-on-derived re-runs even when the id set
  // is unchanged — which is what drops the badge of a child whose start this
  // window never saw. Covers hydration too: a window opening onto an
  // in-progress project session sees its in-flight children on the first pass.
  $effect(() => {
    void refreshChildSessions(runningNoteSessionIds);
  });

  /**
   * Open a child repo session's chat — the dialog the branch card opens for
   * the same session — on top of whatever dialog is showing.
   */
  function openChildSessionChat(sessionId: string) {
    const child = Object.values(childSessionsBySession)
      .flat()
      .find((candidate) => candidate.sessionId === sessionId);
    const branch = child ? branches.find((b) => b.id === child.branchId) : undefined;
    const current = currentDialogReferenceEntry();
    if (current) pushReferenceEntry(current);
    pushReferenceEntry({
      kind: 'chat',
      ref: `#chat:${sessionId}`,
      sessionId,
      branchId: child?.branchId ?? null,
      projectId: project.id,
      repoDir: branch?.worktreePath ?? projectDisplayRootCandidates,
      repoLabel: branch ? repoForBranch(branch) : null,
      hashtagItems,
      diffContext: referenceDiffContext,
    });
    closeReferenceDialogs();
  }

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

  /**
   * Read the stored project draft, settling queued writes first: submit
   * clears the stored draft and close saves immediately through the writer,
   * so reading the store straight away could return the value from before
   * that write lands and resurrect a just-cleared prompt.
   *
   * Never rejects — the modal calls this fire-and-forget on mount, so a
   * failed store read degrades to keeping the seeded draft.
   */
  async function loadStoredProjectDraft(): Promise<string | null> {
    const projectId = project.id;
    try {
      await projectDraftWriter.flush();
      return await loadDraft(projectDraftKey(projectId));
    } catch (err) {
      console.error('[ProjectSection] Failed to load project draft:', err);
      return null;
    }
  }

  async function loadProjectSessionDraft() {
    const promptWhenLoadStarted = draftProjectPrompt;
    if (promptWhenLoadStarted.trim()) return;

    const projectId = project.id;
    const generation = ++projectDraftLoadGeneration;
    const draft = await loadStoredProjectDraft();
    if (generation !== projectDraftLoadGeneration || project.id !== projectId) return;
    if (draftProjectPrompt === promptWhenLoadStarted) {
      draftProjectPrompt = draft ?? '';
    }
  }

  function openProjectSessionModal() {
    // Supersede any in-flight referring open so it can't clobber this one.
    ++projectSessionOpenRequest;
    // Open synchronously seeded with the held draft — the modal fetches the
    // authoritative stored draft itself (via loadDraftForMode) so the click
    // isn't gated on store IPC (a flush plus a read; HTTP hops in web mode).
    showProjectSessionModal = true;
    void ensureHashtagItems();
  }

  function handleProjectSessionDraftChange(
    draft: { prompt: string; imageIds: string[] },
    opts: { immediate?: boolean } = {}
  ) {
    draftProjectPrompt = draft.prompt;
    draftProjectImageIds = draft.imageIds;
    const key = projectDraftKey(project.id);
    if (opts.immediate) {
      void projectDraftWriter.saveNow(key, draft.prompt);
    } else {
      projectDraftWriter.scheduleSave(key, draft.prompt);
    }
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
    void projectDraftWriter.clearNow(projectDraftKey(project.id));
    try {
      const response = await commands.startProjectSession(
        project.id,
        text,
        provider,
        imageIdsToSend,
        data.acpConfigSelection ?? undefined
      );
      // The session's `running` event (emitted before this response) tracks
      // it in activeSessionIds, and the reloaded stub does too. Adding it here
      // as well would re-add a session whose terminal event already landed.
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
  /** Notes whose Resume is in flight, so a second click can't resume twice. */
  let resumingNoteIds = $state<Set<string>>(new Set());
  let confirmDelete = $state<ConfirmDeleteRequest | null>(null);
  /**
   * Orders the note fetches by when they were sent. The list load and the
   * per-session row refetch run concurrently in the backend, so either can
   * land after a fetch sent later than it. A row removed locally once its
   * delete has landed is recorded under `row` too.
   */
  const noteFetchOrder = new LatestWins<'list' | 'row'>();
  /**
   * Per-session latest note refetch, numbered by `noteFetchOrder`, whose
   * counter never resets: an older fetch landing late is dropped, and
   * clearing a landed fetch's entry can't let a later fetch inherit its
   * number.
   */
  const latestNoteRefreshes = new Map<string, number>();

  async function loadProjectNotes(): Promise<void> {
    const ticket = noteFetchOrder.issue();
    try {
      const notes = await commands.listProjectNotes(project.id);
      // A list sent after this one has already landed: this one is older.
      if (!noteFetchOrder.admit('list', ticket)) return;
      // A row refetch sent after this list landed first, so the list may
      // predate what that refetch read. Applying it would put the old row
      // back, and dropping it would lose its other rows: read the list again.
      if (noteFetchOrder.overtaken('row', ticket)) return loadProjectNotes();
      projectNotes = notes;
    } catch (e) {
      console.error('[ProjectSection] Failed to load project notes:', e);
    }
  }

  /** Surgically refetch the note a session writes instead of reloading them all. */
  async function refreshNoteForSession(sessionId: string) {
    const seq = noteFetchOrder.issue();
    latestNoteRefreshes.set(sessionId, seq);
    let updatedNote: ProjectNote | null;
    let superseded = false;
    try {
      updatedNote = await commands.getProjectNoteBySession(sessionId);
    } catch (e) {
      console.error('[ProjectSection] Failed to refresh project note:', e);
      return;
    } finally {
      // A refetch started after this one (the session's terminal event racing
      // its running one, say) owns the row now. Landed or failed, the latest
      // fetch clears its own entry.
      superseded = latestNoteRefreshes.get(sessionId) !== seq;
      if (!superseded) latestNoteRefreshes.delete(sessionId);
    }
    if (superseded) return;
    // A list load sent after this refetch has landed, and read the row later.
    if (noteFetchOrder.overtaken('list', seq)) return;
    noteFetchOrder.admit('row', seq);

    if (updatedNote) {
      projectNotes = projectNotes.map((n) => (n.id === updatedNote.id ? updatedNote : n));
    } else {
      // Note was filtered out (e.g. deleted) — remove from local list
      projectNotes = projectNotes.filter((n) => n.sessionId !== sessionId);
    }
  }

  // Back on screen after home covered this project: the remount this stands
  // in for reloaded the notes on every visit, and this is the one list here
  // that isn't store- or event-backed while mounted.
  onLayerActivated(() => {
    void loadProjectNotes();
  });

  /** Alt-click deletes straight away; otherwise the user confirms first. */
  function handleDeleteNote(noteId: string, opts?: { altKey: boolean }) {
    const note = projectNotes.find((n) => n.id === noteId);
    const sessionId = note?.sessionId ?? undefined;
    const doDelete = async () => {
      confirmDelete = null;
      deletingNoteIds = new Set([...deletingNoteIds, noteId]);
      try {
        await deleteSessionLinkedItem(() => commands.deleteProjectNote(noteId), sessionId);
        // Its child notes take no "Mark as unread", so only the note holds a flag.
        forgetDeletedNote('project', noteId);
        // Optimistic removal; the delete's notes-changed event re-fetches every
        // window's project-note surfaces (including this one) to confirm.
        // A list load still in flight was sent before the delete landed and
        // may have read the note. Counting the removal as a row landing makes
        // that load read the list again instead of putting the row back.
        noteFetchOrder.wrote('row');
        projectNotes = projectNotes.filter((n) => n.id !== noteId);
        hashtagVersion++;
      } catch (e) {
        console.error('[ProjectSection] Failed to delete project note:', e);
        toast.error('Failed to delete project note', {
          description: e instanceof Error ? e.message : String(e),
          duration: Infinity,
        });
      } finally {
        const next = new Set(deletingNoteIds);
        next.delete(noteId);
        deletingNoteIds = next;
      }
    };
    if (opts?.altKey) {
      void doDelete();
      return;
    }
    // Deleting a project note also deletes the notes its session created on
    // branches, so the confirmation names them.
    confirmDelete = {
      title: 'Delete Project Note',
      message: sessionId
        ? 'Delete this project note? Its linked session and the notes it created on branches will also be deleted.'
        : 'Delete this project note?',
      onConfirm: doDelete,
    };
  }

  async function handleResumeNote(note: ProjectNote) {
    const sessionId = note.sessionId;
    if (!sessionId || resumingNoteIds.has(note.id)) return;
    resumingNoteIds = new Set([...resumingNoteIds, note.id]);
    try {
      // Refetch only this note, through the ordering guard: a whole-list
      // reload read while the session ran could land after its terminal
      // refresh and leave the row spinning. The `running` event tracks the id
      // in activeSessionIds; adding it here too could re-add a session whose
      // terminal event beat this response.
      await resumeSessionFromRow(sessionId, null, () => refreshNoteForSession(sessionId));
    } finally {
      const next = new Set(resumingNoteIds);
      next.delete(note.id);
      resumingNoteIds = next;
    }
  }

  /**
   * All notes: completed (oldest last-written first) followed by generating – matches
   * branch timeline order.
   */
  let timelineNotes = $derived(
    [...projectNotes].sort((a, b) => {
      const aIsActive = isSessionActive(a.sessionStatus);
      const bIsActive = isSessionActive(b.sessionStatus);
      if (aIsActive !== bIsActive) return aIsActive ? 1 : -1;
      return noteLastWrittenAt(a) - noteLastWrittenAt(b);
    })
  );
  let projectNoteContextMenuActions = $derived.by(() => {
    const actions: TimelineContextMenuAction[] = [];
    for (const note of timelineNotes) {
      const deleting = deletingNoteIds.has(note.id);
      const readState = deleting ? null : projectNoteReadState(note);
      actions.push({
        key: projectNoteContextMenuKey(note),
        hashtagRef: isCompletedProjectNote(note) ? `#project-note:${note.id}` : undefined,
        onMarkRead: readState === 'unread' ? () => setProjectNoteRead(note.id, true) : undefined,
        onMarkUnread: readState === 'read' ? () => setProjectNoteRead(note.id, false) : undefined,
        onDelete: deleting ? undefined : (opts) => handleDeleteNote(note.id, opts),
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
  let childHashtagItems = $state<HashtagItem[]>([]);
  // Every child, titled or not: a move takes them all, so the picker's hint
  // and its project list depend on there being any. Kept current with the
  // hashtag items while the dialog is open: a reply can resume the session
  // and spawn a child, and a branch move can detach one (see
  // `refreshOpenNoteChildren`).
  let openNoteChildCount = $state(0);
  let childHashtagLoadGeneration = 0;
  let noteModalHashtagItems = $derived([...hashtagItems, ...childHashtagItems]);
  // Live, not the snapshot `openNote` was opened with: the session can be
  // resumed from the dialog itself, and "Move" has to follow.
  let openNoteSessionStatus = $derived.by(() => {
    const sid = openNote?.sessionId;
    if (!sid) return null;
    if (runningNoteSessionIds.includes(sid)) return 'running';
    return projectNotes.find((n) => n.id === openNote?.noteId)?.sessionStatus ?? null;
  });

  // Off screen (home shown over this project, or parked under settings / the
  // diff view), this section's dialogs would stay open on top. The composer
  // hands its draft back on the way out; closing the note releases its pooled
  // chat pane.
  onViewHidden(() => {
    showProjectSessionModal = false;
    confirmDelete = null;
    if (openNote) {
      closeOpenNote();
      void loadProjectNotes();
    }
  });

  $effect(() => {
    const _signature = hashtagSignature;
    if (!openNote) return;
    void ensureHashtagItems();
  });

  /**
   * Adapt a fetched note into a hashtag item so `#note:<id>` references
   * resolve. Used for a project note's children and for notes fetched on
   * demand when a reference isn't in the merged item list.
   */
  function noteToHashtagItem(note: NoteTimelineItem): HashtagItem {
    return {
      type: 'note',
      id: note.id,
      title: note.title,
      color: '--note-color',
      bgColor: '--note-bg',
      projectId: project.id,
      noteContent: note.content,
      noteSessionId: note.sessionId,
      noteUpdatedAt: note.updatedAt,
    };
  }

  /**
   * Load the open note's children. On the first load a failure leaves the
   * (already empty) state alone; on a `refresh` it keeps what loaded before,
   * since stale-but-right beats blank, and only logs.
   */
  async function loadChildHashtagItems(parentProjectNoteId: string, refresh = false) {
    const generation = ++childHashtagLoadGeneration;
    try {
      const children = await commands.listChildNotes(parentProjectNoteId);
      if (generation !== childHashtagLoadGeneration) return;
      if (openNote?.noteId !== parentProjectNoteId) return;
      openNoteChildCount = children.length;
      childHashtagItems = children.filter((note) => note.title.trim()).map(noteToHashtagItem);
    } catch (e) {
      if (generation !== childHashtagLoadGeneration) return;
      if (openNote?.noteId !== parentProjectNoteId) return;
      console.error('[ProjectSection] Failed to load child notes:', e);
      if (!refresh) {
        openNoteChildCount = 0;
        childHashtagItems = [];
      }
    }
  }

  /**
   * Re-read the open note's children, so the `#note:` references and the
   * move picker see the same children the backend would move. Called when
   * the note's session leaves an active status (a resumed session may have
   * spawned children), on the project-notes invalidation a branch move
   * raises when it detaches children, and on the timeline invalidation a
   * child's own create, delete or move raises (those publish branch-scoped
   * changes only).
   */
  function refreshOpenNoteChildren() {
    if (openNote) void loadChildHashtagItems(openNote.noteId, true);
  }

  function isCompletedProjectNote(note: ProjectNote): boolean {
    return noteRowPhase(note) === 'done';
  }

  function projectNoteReadState(note: ProjectNote) {
    return isCompletedProjectNote(note) ? noteReadState(note) : null;
  }

  function setProjectNoteRead(noteId: string, read: boolean) {
    void setNoteReadState({ kind: 'project', noteId }, read);
  }

  function projectNoteContextMenuKey(note: ProjectNote): string {
    return `project-note-${note.id}`;
  }

  async function handleProjectNoteNewSessionReferring(ref: string) {
    const request = ++projectSessionOpenRequest;
    await loadProjectSessionDraft();
    // A plain open landed during the read: the modal is already up without
    // the ref and only reads initialPrompt once, so saving the ref-appended
    // prompt would just be overwritten by its next keystroke. Drop the ref,
    // as BranchCardSessionManager.openNewSessionReferring does.
    if (request !== projectSessionOpenRequest) return;
    draftProjectPrompt = buildReferringPrompt(draftProjectPrompt, ref);
    showProjectSessionModal = true;
    void ensureHashtagItems();
    // Keep this save in the same tick as opening the modal so its
    // loadInitialDraft hits the equality fast path (stored draft === seed)
    // and the ref is preserved exactly. An await here wouldn't lose the ref,
    // but the open would then depend on the insertedText prefix/suffix
    // heuristic instead.
    void projectDraftWriter.saveNow(projectDraftKey(project.id), draftProjectPrompt);
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
    childHashtagItems = [];
    openNoteChildCount = 0;
    void ensureHashtagItems();
    void loadChildHashtagItems(note.id);
  }

  function closeOpenNote() {
    openNote = null;
    childHashtagItems = [];
    openNoteChildCount = 0;
    childHashtagLoadGeneration++;
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
        hashtagItems: noteModalHashtagItems,
        diffContext: referenceDiffContext,
      };
    }

    return null;
  }

  function closeReferenceDialogs() {
    closeOpenNote();
  }

  function handleHashtagClick(click: HashtagClickInfo) {
    const target = resolveHashtagReference(click, {
      hashtagItems: noteModalHashtagItems,
      diffContext: referenceDiffContext,
    });
    if (target) {
      openHashtagTarget(target);
      return;
    }

    // A note reference can point outside the merged item list — e.g. a child of
    // a different project note, or another project's project note (branch
    // history renders a child as "of project note #project-note:<id>", which an
    // agent may quote into a body read from here). Fetch it directly so the
    // reference opens instead of silently no-opping.
    if (click.type === 'note' || click.type === 'project-note') {
      void openUnresolvedNoteReference(click);
    }
  }

  function openHashtagTarget(target: ReferenceHistoryEntry) {
    const current = currentDialogReferenceEntry();
    if (current) pushReferenceEntry(current);
    pushReferenceEntry(target);
    closeReferenceDialogs();
    if (target.kind === 'diff') {
      openDiffRoute(target.route);
    }
  }

  /** Fetch the note behind an unresolved reference as a hashtag item. */
  async function fetchUnresolvedNoteItem(click: HashtagClickInfo): Promise<HashtagItem | null> {
    if (click.type === 'note') {
      const note = await commands.getNote(click.id);
      if (note) return noteToHashtagItem(note);
    }
    // `#note:<id>` also accepts a project note (see the lookup aliases in
    // hashtagItems), so both kinds fall through to the project-note lookup.
    const projectNote = await commands.getProjectNote(click.id);
    return projectNote ? (projectNotesToHashtagItems([projectNote]).at(0) ?? null) : null;
  }

  async function openUnresolvedNoteReference(click: HashtagClickInfo) {
    const originNoteId = openNote?.noteId;
    let item: HashtagItem | null;
    try {
      item = await fetchUnresolvedNoteItem(click);
    } catch (e) {
      console.error('[ProjectSection] Failed to load referenced note:', e);
      return;
    }
    if (!item) return;
    // The dialog the click came from may have closed or moved on while the
    // fetch was in flight; opening now would push a stale back-stack entry.
    if (openNote?.noteId !== originNoteId) return;

    const target = resolveHashtagReference(
      { ...click, item },
      { hashtagItems: noteModalHashtagItems, diffContext: referenceDiffContext }
    );
    if (target) openHashtagTarget(target);
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
    const onTimelineInvalidated = (event: Event) => {
      hashtagVersion++;
      refreshOpenNoteChildren();

      // Queuing a child through `start_repo_session` publishes no session
      // event — the queued row is a session-family write — but its artifact
      // stub lands on a branch of this project, and that change reaches
      // every window as this event. Re-run the child query so the queued
      // child badges at once rather than when the branch queue gets to it.
      // The fresh Set is the trigger (see the badge `$effect`); its contents
      // are unchanged.
      if (runningNoteSessionIds.length === 0) return;
      const branchIds = (event as CustomEvent<{ branchIds: string[] | null }>).detail?.branchIds;
      if (branchIds !== null && !branchIds?.some((id) => branches.some((b) => b.id === id))) {
        return;
      }
      activeSessionIds = new Set(activeSessionIds);
    };
    window.addEventListener('timeline-invalidated', onTimelineInvalidated);

    // Re-fetch when a project-note write lands anywhere — another window, or
    // this one's own edit echoing back through notes-changed.
    const onProjectNotesInvalidated = () => {
      void loadProjectNotes();
      refreshOpenNoteChildren();
      hashtagVersion++;
    };
    window.addEventListener('project-notes-invalidated', onProjectNotesInvalidated);

    const unlistenSession = listenToEvent<{
      sessionId: string;
      status: string;
      projectId?: string;
      branchId?: string | null;
    }>('session-status-changed', async (payload) => {
      const { sessionId, status, projectId, branchId } = payload;
      if (projectId !== project.id) return;

      if (status === 'running') {
        // Bridge: track until the stub (already loaded by startProjectSession) is
        // updated with an authoritative sessionStatus on the terminal event.
        // This may be a repo session the queue just drained on a parent's
        // behalf (`branchId` set); its id stays out of the set, but the
        // fresh-Set reassignment is what makes its badge appear.
        activeSessionIds = nextActiveSessionIds(activeSessionIds, payload);

        if (!branchId) {
          const listed = projectNotes.find((n) => n.sessionId === sessionId);
          if (!listed) {
            // A project session started outside this client (stagedctl)
            // created a note stub we haven't loaded. Branch sessions carry a
            // branchId; project sessions never do.
            await loadProjectNotes();
          } else if (!isSessionActive(listed.sessionStatus)) {
            // A listed note whose session starts running again was resumed —
            // from the note's chat, a row's Resume, or another window. Its row
            // still holds the finished status, so refetch it to show it
            // running.
            await refreshNoteForSession(sessionId);
          }
        }
        return;
      }

      if (
        status === 'completed' ||
        status === 'paused' ||
        status === 'error' ||
        status === 'cancelled'
      ) {
        // A finished child drops its badge; a finished parent drops the row.
        activeSessionIds = nextActiveSessionIds(activeSessionIds, payload);

        await refreshNoteForSession(sessionId);

        // Branch timelines were already invalidated by sessionStatusListener
        // for the session's own branch; just re-derive the hashtag items.
        hashtagVersion++;
        if (openNote?.sessionId === sessionId) refreshOpenNoteChildren();
      }
    });

    return () => {
      unlistenSession();
      window.removeEventListener('timeline-invalidated', onTimelineInvalidated);
      window.removeEventListener('project-notes-invalidated', onProjectNotesInvalidated);
    };
  });
</script>

<div class="project-section">
  <section class="project-overview-card">
    <div class="project-heading">
      <div class="project-title-row">
        <!--
          The heading stays mounted while editing so the element reported to
          ProjectHome (which measures it to decide when the top bar shows the
          project name) keeps a stable identity. The pencil is a sibling rather
          than a child so the heading's accessible name is just the project name.
        -->
        <h2 class="project-title" class:editing={editingProjectName} use:reportProjectTitleElement>
          {#if editingProjectName}
            <span class="project-name-editor">
              <input
                bind:this={projectNameInput}
                class="project-name-input"
                class:error={!!renameProjectError}
                bind:value={projectNameDraft}
                disabled={renameProjectSaving || deleting}
                aria-label="Project name"
                aria-invalid={!!renameProjectError}
                title={renameProjectError ?? 'Project name'}
                onkeydown={handleProjectNameKeydown}
                onblur={() => void saveProjectRename('blur')}
              />
              {#if renameProjectSaving}
                <Spinner size={12} />
              {/if}
            </span>
          {:else}
            <span class="project-title-text" title={projectDisplayName(project)}
              >{projectDisplayName(project)}</span
            >
          {/if}
        </h2>
        {#if !editingProjectName}
          <button
            class="rename-project-button"
            onclick={() => void startProjectRename()}
            disabled={deleting || renameProjectSaving || !onRenameProject}
            title="Rename project"
            aria-label="Rename project"
          >
            <Pencil size={14} />
          </button>
        {/if}
      </div>
      <ProjectTagPills {project} {branches} disabled={deleting} pickerRequest={tagPickerRequest} />
    </div>
    {#if renameProjectError}
      <span class="project-name-error" role="alert">{renameProjectError}</span>
    {/if}

    {#if projectNotes.length > 0}
      {@const nowMs = minuteNow.now()}
      <TimelineContextMenu
        actions={projectNoteContextMenuActions}
        onNewSessionReferring={handleProjectNoteNewSessionReferring}
      >
        <div class="notes-timeline">
          {#each timelineNotes as note, index (note.id)}
            {@const isRunning = isSessionActive(note.sessionStatus)}
            {@const isDeleting = deletingNoteIds.has(note.id)}
            {@const liveHint =
              isRunning && note.sessionId ? liveSessionHints[note.sessionId] : undefined}
            {@const liveTitle =
              isRunning && note.sessionId ? liveSessionTitles[note.sessionId] : undefined}
            {@const childBadges = buildChildSessionBadges(
              (isRunning && note.sessionId ? childSessionsBySession[note.sessionId] : undefined) ??
                [],
              liveSessionTitles
            )}
            {@const row = projectNoteRow(note, {
              deleting: isDeleting,
              nowMs,
              liveTitle,
              liveHint,
              childBadges,
            })}
            <TimelineRow
              type={row.type}
              title={row.title}
              secondaryMeta={row.secondaryMeta}
              childSessionBadges={childBadges}
              onChildSessionClick={openChildSessionChat}
              deleting={isDeleting}
              isLast={false}
              sessionId={note.sessionId ?? undefined}
              chatState={row.type === 'note' ? noteChatState(note) : 'none'}
              unread={projectNoteReadState(note) === 'unread'}
              hasDraft={row.type === 'note' && replyDraftPresence.has(note.sessionId)}
              onItemClick={row.type === 'note' ? () => openProjectNote(note) : undefined}
              onSessionClick={(sid) => {
                openProjectNote({ ...note, sessionId: sid }, true);
              }}
              deleteDisabledReason={isDeleting ? 'Deleting...' : undefined}
              onDeleteClick={(opts) => handleDeleteNote(note.id, opts)}
              onResumeClick={canResumeRow({
                sessionId: note.sessionId,
                completionReason: note.completionReason,
                deleting: isDeleting,
                phase: noteRowPhase(note),
              }) && !resumingNoteIds.has(note.id)
                ? () => handleResumeNote(note)
                : undefined}
              resumeLabel={note.completionReason === 'rate_limited' ? 'Resume now' : undefined}
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

  <div
    class="branches-list"
    class:deleting
    class:transferring
    inert={deleting}
    aria-busy={transferring || undefined}
  >
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
        onMove={(targetProjectId) => onMoveBranch?.(branch.id, targetProjectId)}
        onRetryWorktree={() => onRetryWorktree?.(branch.id)}
      />
    {/each}
  </div>

  {#if !transferring}
    <SuggestedRepos {project} {reposById} {onRepoSelected} />
  {/if}
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
    loadDraftForMode={() => loadStoredProjectDraft()}
    onDraftChange={(draft, opts) => {
      handleProjectSessionDraftChange(draft, opts);
    }}
    onClose={(draft) => {
      handleProjectSessionDraftChange(draft, { immediate: true });
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

<ConfirmDeleteDialog request={confirmDelete} onCancel={() => (confirmDelete = null)} />

{#if openNote}
  <NoteModal
    open={true}
    title={openNote.title}
    content={openNote.content}
    sessionId={openNote.sessionId}
    sessionStatus={openNoteSessionStatus}
    hasChildren={openNoteChildCount > 0}
    noteUpdatedAt={openNote.noteUpdatedAt}
    noteId={openNote.noteId}
    noteKind="project"
    projectId={project.id}
    repoDir={projectDisplayRootCandidates}
    chatOpen={openNote.chatOpen ?? false}
    onChatOpenChange={(chatOpen) => {
      if (openNote) openNote = { ...openNote, chatOpen };
    }}
    hashtagItems={noteModalHashtagItems}
    referenceNav={disabledReferenceNav}
    onClose={() => {
      closeOpenNote();
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

  /* The title, with the tag pills on their own row under it so a long title
     isn't squeezed by them. */
  .project-heading {
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-width: 0;
  }

  /* The title sizes to its text with the pencil hugging it; a long title
     wraps within its column. */
  .project-title-row {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
  }

  .project-title {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-wrap: wrap;
    flex: 0 1 auto;
    min-width: 0;
    margin: 0;
    color: var(--text-primary);
    font-size: 22px;
    font-weight: 700;
    line-height: 1.2;
    overflow-wrap: anywhere;
  }

  /* While editing, the heading holds the input, which grows into the free
     space up to the editor's cap. */
  .project-title.editing {
    flex: 1 1 auto;
  }

  .project-title-text {
    min-width: 0;
  }

  /* Pull the pencil in from the row gap so it sits 4px after the title text,
     as it did when it was the heading's last child. */
  .rename-project-button {
    margin-left: -8px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 26px;
    padding: 0;
    border: none;
    border-radius: 6px;
    background-color: transparent;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.15s ease,
      background-color 0.15s ease;
    flex-shrink: 0;
  }

  .rename-project-button:hover:not(:disabled) {
    color: var(--text-primary);
    background-color: var(--ui-selection);
  }

  .rename-project-button:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }

  .project-name-editor {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 160px;
    max-width: min(420px, 45vw);
    flex: 1 1 auto;
  }

  .project-name-input {
    width: 100%;
    min-width: 0;
    padding: 2px 8px;
    border: 1px solid var(--border-emphasis);
    border-radius: 6px;
    outline: none;
    background-color: var(--bg-primary);
    color: inherit;
    /* Inherit the heading's type so the editor matches the title it replaces,
       including the narrow-viewport font-size override below. */
    font-family: inherit;
    font-size: inherit;
    font-weight: inherit;
    line-height: inherit;
  }

  .project-name-input:focus {
    border-color: var(--ui-accent);
  }

  .project-name-input.error {
    border-color: var(--ui-danger);
  }

  .project-name-input:disabled {
    opacity: 0.65;
  }

  .project-name-error {
    color: var(--ui-danger);
    font-size: var(--size-xs);
    font-weight: 500;
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

  /* A taller New-note control on a touch screen (as in BranchTimeline's
     footer-left-actions). */
  @media (pointer: coarse) {
    .project-session-footer :global(button) {
      min-height: 36px;
    }
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

  /* A transferring project's branches stay usable: new sessions and replies
     queue behind the transfer (the banner above says so), so the list is
     only dimmed a little rather than taken out of reach. */
  .branches-list.transferring {
    opacity: 0.85;
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
