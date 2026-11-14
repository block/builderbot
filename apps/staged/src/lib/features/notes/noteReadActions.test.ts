import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch, BranchNote, ProjectNote } from '../../types';

const { commands, deletingProjectIds, loadedBranches, noteBranches, sharedStore, toastError } =
  vi.hoisted(() => ({
    commands: {
      setNoteRead: vi.fn<(noteId: string, read: boolean) => Promise<BranchNote>>(),
      setProjectNoteRead: vi.fn<(noteId: string, read: boolean) => Promise<ProjectNote>>(),
      getBranch: vi.fn<(branchId: string) => Promise<Branch | null>>(),
    },
    deletingProjectIds: new Set<string>(),
    /** Project id -> ids of the branches the projects store has loaded for it. */
    loadedBranches: new Map<string, string[]>(),
    /** Note id -> the branch a branch note is on. */
    noteBranches: new Map<string, string>(),
    /**
     * The persistent store every window shares: a write notifies every
     * window's listener for the key, the writer's included, on a later tick.
     */
    sharedStore: {
      values: new Map<string, unknown>(),
      listeners: new Set<{ key: string; callback: (value: unknown) => void }>(),
    },
    toastError: vi.fn(),
  }));

vi.mock('../../api/commands', () => commands);
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    isProjectDeleting: (id: string) => deletingProjectIds.has(id),
    get branchesByProject() {
      return new Map(
        [...loadedBranches].map(([projectId, ids]) => [projectId, ids.map((id) => ({ id }))])
      );
    },
  },
}));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));
// The real project state store, minus the dock badge.
vi.mock('../../transport', () => ({
  getWindowSync: () => ({ setBadgeCount: async () => {} }),
}));
vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: async (key: string) => structuredClone(sharedStore.values.get(key)),
  setStoreValue: async (key: string, value: unknown) => {
    sharedStore.values.set(key, structuredClone(value));
    await Promise.resolve();
    for (const listener of sharedStore.listeners) {
      if (listener.key === key) listener.callback(structuredClone(value));
    }
  },
  listenToStoreKeyChange: (
    key: string,
    callback: (value: unknown) => void,
    options?: { onEstablished?: () => void }
  ) => {
    const listener = { key, callback };
    sharedStore.listeners.add(listener);
    options?.onEstablished?.();
    return () => sharedStore.listeners.delete(listener);
  },
}));
vi.mock('../../stores/sessionRegistry.svelte', () => ({
  sessionRegistry: { getProjectId: () => null },
}));

type Actions = typeof import('./noteReadActions');
type ProjectStateStore = typeof import('../../stores/projectState.svelte').projectStateStore;
type Tracker = import('./noteReadTracker.svelte').NoteReadTracker;

/** One app window: its own module instances, sharing the persistent store. */
interface AppWindow extends Actions {
  projectStateStore: ProjectStateStore;
  newTracker: () => Tracker;
}

let setNoteReadState: Actions['setNoteReadState'];
let writeNoteRead: Actions['writeNoteRead'];
let forgetDeletedNote: Actions['forgetDeletedNote'];
let projectStateStore: ProjectStateStore;
let newTracker: () => Tracker;

async function openWindow(): Promise<AppWindow> {
  vi.resetModules();
  const actions = await import('./noteReadActions');
  const { projectStateStore } = await import('../../stores/projectState.svelte');
  const { NoteReadTracker } = await import('./noteReadTracker.svelte');
  await projectStateStore.initFromStore();
  return { ...actions, projectStateStore, newTracker: () => new NoteReadTracker() };
}

function branchNote(overrides: Partial<BranchNote> = {}): BranchNote {
  return {
    id: 'note-1',
    branchId: 'branch-1',
    sessionId: null,
    title: 'Note',
    content: 'Body',
    createdAt: 1,
    updatedAt: 1,
    completedAt: 1,
    suggestedNextCommitStep: null,
    suggestedNextNoteStep: null,
    parentProjectNoteId: null,
    subtype: null,
    readAt: null,
    ...overrides,
  };
}

function projectNote(overrides: Partial<ProjectNote> = {}): ProjectNote {
  return {
    id: 'project-note-1',
    projectId: 'project-of-note',
    sessionId: null,
    title: 'Project note',
    content: 'Body',
    createdAt: 1,
    updatedAt: 1,
    completedAt: 1,
    suggestedNextCommitStep: null,
    suggestedNextNoteStep: null,
    sessionStatus: null,
    completionReason: null,
    lastChatMessageAt: null,
    chatReadAt: null,
    readAt: null,
    ...overrides,
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(async () => {
  vi.clearAllMocks();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  deletingProjectIds.clear();
  loadedBranches.clear();
  noteBranches.clear();
  sharedStore.values.clear();
  sharedStore.listeners.clear();
  commands.setNoteRead.mockImplementation(async (id, read) =>
    branchNote({ id, branchId: noteBranches.get(id) ?? 'branch-1', readAt: read ? 2 : null })
  );
  commands.setProjectNoteRead.mockImplementation(async (id, read) =>
    projectNote({ id, readAt: read ? 2 : null })
  );
  commands.getBranch.mockResolvedValue(null);
  // A fresh project state store per test.
  ({ setNoteReadState, writeNoteRead, forgetDeletedNote, projectStateStore, newTracker } =
    await openWindow());
});

/** Put a branch note on a branch of `projectId`, which the projects store has loaded. */
function placeBranchNote(noteId: string, projectId: string) {
  const branchId = `branch-of-${projectId}`;
  noteBranches.set(noteId, branchId);
  const branches = loadedBranches.get(projectId) ?? [];
  if (!branches.includes(branchId)) loadedBranches.set(projectId, [...branches, branchId]);
}

/** Mark a branch note in `projectId` read or unread from a timeline menu. */
function setBranchNote(noteId: string, projectId: string, read: boolean, appWindow?: AppWindow) {
  placeBranchNote(noteId, projectId);
  return (appWindow?.setNoteReadState ?? setNoteReadState)({ kind: 'branch', noteId }, read);
}

describe('writeNoteRead', () => {
  it('writes through the command for the note kind', async () => {
    await writeNoteRead('branch', 'note-1', true);
    await writeNoteRead('project', 'project-note-1', false);

    expect(commands.setNoteRead).toHaveBeenCalledWith('note-1', true);
    expect(commands.setProjectNoteRead).toHaveBeenCalledWith('project-note-1', false);
    expect(projectStateStore.isUnread('project-of-note')).toBe(false);
  });
});

describe('setNoteReadState', () => {
  it('marks a branch note’s project unread along with the note', async () => {
    const row = await setBranchNote('note-1', 'project-1', false);

    expect(row?.readAt).toBeNull();
    expect(projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('marks the project a project note names unread', async () => {
    await setNoteReadState({ kind: 'project', noteId: 'project-note-1' }, false);

    expect(projectStateStore.isUnread('project-of-note')).toBe(true);
  });

  it('flags the project a branch note is in, looking up a branch no view has loaded', async () => {
    // A `#note:<id>` reference opened from another project's view.
    loadedBranches.set('project-on-screen', ['branch-on-screen']);
    noteBranches.set('note-1', 'elsewhere-branch');
    commands.getBranch.mockResolvedValue({ projectId: 'project-elsewhere' } as Branch);
    const tracker = newTracker();
    tracker.show('branch', 'note-1');
    await flush();

    await tracker.toggle();

    expect(commands.getBranch).toHaveBeenCalledWith('elsewhere-branch');
    expect(projectStateStore.isUnread('project-elsewhere')).toBe(true);
    expect(projectStateStore.isUnread('project-on-screen')).toBe(false);
  });

  it('finds a loaded branch’s project without a lookup', async () => {
    await setBranchNote('note-1', 'project-1', false);

    expect(commands.getBranch).not.toHaveBeenCalled();
    expect(projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('flags no project when the note’s branch can’t be found', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    noteBranches.set('note-1', 'gone-branch');
    noteBranches.set('note-2', 'unreachable-branch');
    commands.getBranch.mockResolvedValueOnce(null);
    commands.getBranch.mockRejectedValueOnce(new Error('backend down'));

    const gone = await setNoteReadState({ kind: 'branch', noteId: 'note-1' }, false);
    const unreachable = await setNoteReadState({ kind: 'branch', noteId: 'note-2' }, false);

    // The note itself is still marked unread.
    expect(gone?.readAt).toBeNull();
    expect(unreachable?.readAt).toBeNull();
    expect(projectStateStore.getUnreadCount()).toBe(0);
    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });

  describe('while the note’s project is being looked up', () => {
    /** Mark a note unread whose branch no view has loaded, holding the lookup open. */
    async function markUnreadHoldingLookup() {
      noteBranches.set('note-1', 'elsewhere-branch');
      let answerLookup!: (branch: Branch | null) => void;
      commands.getBranch.mockImplementationOnce(
        () => new Promise<Branch | null>((resolve) => (answerLookup = resolve))
      );
      const marked = setNoteReadState({ kind: 'branch', noteId: 'note-1' }, false);
      await flush();
      expect(commands.getBranch).toHaveBeenCalledTimes(1);
      return {
        marked,
        answerLookup: () => answerLookup({ projectId: 'project-elsewhere' } as Branch),
      };
    }

    it('does not flag the project of a note that was read in the meantime', async () => {
      const { marked, answerLookup } = await markUnreadHoldingLookup();

      // The viewer opens the note: nothing is flagged yet for it to release.
      await writeNoteRead('branch', 'note-1', true);
      answerLookup();
      await marked;

      expect(projectStateStore.isUnread('project-elsewhere')).toBe(false);
    });

    it('does not flag the project of a note that was deleted in the meantime', async () => {
      const { marked, answerLookup } = await markUnreadHoldingLookup();

      forgetDeletedNote('branch', 'note-1');
      answerLookup();
      await marked;

      expect(projectStateStore.isUnread('project-elsewhere')).toBe(false);
    });

    it('still flags it when the note is marked unread again in the meantime', async () => {
      const { marked, answerLookup } = await markUnreadHoldingLookup();

      commands.getBranch.mockResolvedValue({ projectId: 'project-elsewhere' } as Branch);
      await setNoteReadState({ kind: 'branch', noteId: 'note-1' }, false);
      answerLookup();
      await marked;

      expect(projectStateStore.isUnread('project-elsewhere')).toBe(true);
      // One hold, released by one read.
      await writeNoteRead('branch', 'note-1', true);
      expect(projectStateStore.isUnread('project-elsewhere')).toBe(false);
    });
  });

  it('clears the project flag when the note is marked read again', async () => {
    await setBranchNote('note-1', 'project-1', false);

    const row = await setBranchNote('note-1', 'project-1', true);

    expect(row?.readAt).toBe(2);
    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });

  it('keeps the project flagged while another note marked unread holds it', async () => {
    await setBranchNote('note-1', 'project-1', false);
    await setBranchNote('note-2', 'project-1', false);

    await setBranchNote('note-1', 'project-1', true);
    expect(projectStateStore.isUnread('project-1')).toBe(true);

    await setBranchNote('note-2', 'project-1', true);
    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });

  it('clears the project flag when the viewer shows the note', async () => {
    await setBranchNote('note-1', 'project-1', false);

    newTracker().show('branch', 'note-1');
    await flush();

    expect(commands.setNoteRead).toHaveBeenLastCalledWith('note-1', true);
    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });

  it('leaves a flag set some other way to the project', async () => {
    // Flagged from the sidebar, or by a session, after the note…
    await setBranchNote('note-1', 'project-1', false);
    projectStateStore.markAsUnread('project-1');
    await setBranchNote('note-1', 'project-1', true);
    expect(projectStateStore.isUnread('project-1')).toBe(true);

    // …or before it.
    projectStateStore.markAsUnread('project-2');
    await setBranchNote('note-2', 'project-2', false);
    await setBranchNote('note-2', 'project-2', true);
    expect(projectStateStore.isUnread('project-2')).toBe(true);
  });

  it('forgets the notes once the project is opened', async () => {
    await setBranchNote('note-1', 'project-1', false);
    projectStateStore.markAsRead('project-1');
    await setBranchNote('note-2', 'project-1', false);
    expect(projectStateStore.isUnread('project-1')).toBe(true);

    // Only the note marked unread since then holds the flag.
    newTracker().show('branch', 'note-1');
    await flush();
    expect(projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('leaves a project being deleted alone', async () => {
    deletingProjectIds.add('project-1');
    await setBranchNote('note-1', 'project-1', false);
    expect(projectStateStore.isUnread('project-1')).toBe(false);

    // Flagged before the delete started, a read leaves it to the delete.
    deletingProjectIds.clear();
    await setBranchNote('note-1', 'project-1', false);
    deletingProjectIds.add('project-1');
    await setBranchNote('note-1', 'project-1', true);

    expect(commands.setNoteRead).toHaveBeenCalledTimes(3);
    expect(projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('toasts a failed write and leaves the project alone', async () => {
    commands.setNoteRead.mockRejectedValueOnce(new Error('Note not found: note-1'));

    const row = await setBranchNote('note-1', 'project-1', false);

    expect(row).toBeNull();
    expect(toastError).toHaveBeenCalledWith('Unable to mark note as unread', {
      description: 'Note not found: note-1',
    });
    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });
});

describe('forgetDeletedNote', () => {
  it('clears the project flag once every note holding it is deleted', async () => {
    await setBranchNote('note-1', 'project-1', false);
    await setNoteReadState({ kind: 'project', noteId: 'project-note-1' }, false);
    await setBranchNote('note-2', 'project-of-note', false);

    forgetDeletedNote('branch', 'note-1');
    expect(projectStateStore.isUnread('project-1')).toBe(false);

    forgetDeletedNote('project', 'project-note-1');
    expect(projectStateStore.isUnread('project-of-note')).toBe(true);
    forgetDeletedNote('branch', 'note-2');
    expect(projectStateStore.isUnread('project-of-note')).toBe(false);
  });

  it('only releases the deleted note’s own hold', async () => {
    await setBranchNote('note-1', 'project-1', false);

    // A project note with the same id, and a note that holds nothing.
    forgetDeletedNote('project', 'note-1');
    forgetDeletedNote('branch', 'note-3');

    expect(projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('leaves a flag set some other way to the project', async () => {
    await setBranchNote('note-1', 'project-1', false);
    projectStateStore.markAsUnread('project-1');

    forgetDeletedNote('branch', 'note-1');

    expect(projectStateStore.isUnread('project-1')).toBe(true);
  });
});

describe('across windows', () => {
  it('flags the project in every window', async () => {
    const other = await openWindow();

    await setBranchNote('note-1', 'project-1', false);
    await flush();

    expect(projectStateStore.isUnread('project-1')).toBe(true);
    expect(other.projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('clears the flag everywhere when another window reads the note', async () => {
    const other = await openWindow();
    await setBranchNote('note-1', 'project-1', false);
    await flush();

    other.newTracker().show('branch', 'note-1');
    await flush();

    expect(other.projectStateStore.isUnread('project-1')).toBe(false);
    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });

  it('clears the flag everywhere when another window deletes the note', async () => {
    const other = await openWindow();
    await setBranchNote('note-1', 'project-1', false);
    await flush();

    other.forgetDeletedNote('branch', 'note-1');
    await flush();

    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });

  it('keeps the flag while a note marked unread in another window still holds it', async () => {
    const other = await openWindow();
    await setBranchNote('note-1', 'project-1', false);
    await setBranchNote('note-2', 'project-1', false, other);
    await flush();

    await setBranchNote('note-1', 'project-1', true);
    await flush();

    expect(projectStateStore.isUnread('project-1')).toBe(true);
    expect(other.projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('leaves a window’s own flag alone when the note is read elsewhere', async () => {
    const other = await openWindow();
    // A session finished in the project while the other window was elsewhere.
    other.projectStateStore.markAsUnread('project-1');
    await setBranchNote('note-1', 'project-1', false);
    await flush();

    await setBranchNote('note-1', 'project-1', true);
    await flush();

    expect(projectStateStore.isUnread('project-1')).toBe(false);
    expect(other.projectStateStore.isUnread('project-1')).toBe(true);
  });

  it('drops the notes in every window once one opens the project', async () => {
    const other = await openWindow();
    await setBranchNote('note-1', 'project-1', false);
    await flush();

    other.projectStateStore.markAsRead('project-1');
    await flush();

    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });

  it('hands the notes to a window opened later, or after a restart', async () => {
    await setBranchNote('note-1', 'project-1', false);
    await flush();

    const later = await openWindow();
    expect(later.projectStateStore.isUnread('project-1')).toBe(true);

    later.newTracker().show('branch', 'note-1');
    await flush();
    expect(later.projectStateStore.isUnread('project-1')).toBe(false);
    expect(projectStateStore.isUnread('project-1')).toBe(false);
  });

  it('prunes the notes of deleted projects from every window', async () => {
    const other = await openWindow();
    await setBranchNote('note-1', 'project-1', false);
    await setBranchNote('note-2', 'deleted-project', false);
    await flush();

    await other.projectStateStore.pruneDeletedProjects(new Set(['project-1']));
    await flush();

    expect(projectStateStore.isUnread('deleted-project')).toBe(false);
    expect(projectStateStore.isUnread('project-1')).toBe(true);
    expect(sharedStore.values.get('note-unread-holds')).toEqual({ 'project-1': ['branch:note-1'] });
  });
});
