import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BranchNote, ProjectNote } from '../../types';

const { commands, deletingProjectIds, toastError } = vi.hoisted(() => ({
  commands: {
    setNoteRead: vi.fn<(noteId: string, read: boolean) => Promise<BranchNote>>(),
    setProjectNoteRead: vi.fn<(noteId: string, read: boolean) => Promise<ProjectNote>>(),
  },
  deletingProjectIds: new Set<string>(),
  toastError: vi.fn(),
}));

vi.mock('../../api/commands', () => commands);
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: { isProjectDeleting: (id: string) => deletingProjectIds.has(id) },
}));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));
// The real project state store, minus persistence and the dock badge.
vi.mock('../../transport', () => ({
  getWindowSync: () => ({ setBadgeCount: async () => {} }),
}));
vi.mock('../../shared/persistentStore', () => ({
  getStoreValue: async () => undefined,
  setStoreValue: async () => {},
}));
vi.mock('../../stores/sessionRegistry.svelte', () => ({
  sessionRegistry: { getProjectId: () => null },
}));

type Actions = typeof import('./noteReadActions');
type ProjectStateStore = typeof import('../../stores/projectState.svelte').projectStateStore;
type Tracker = import('./noteReadTracker.svelte').NoteReadTracker;

let setNoteReadState: Actions['setNoteReadState'];
let writeNoteRead: Actions['writeNoteRead'];
let projectStateStore: ProjectStateStore;
let newTracker: () => Tracker;

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
  commands.setNoteRead.mockImplementation(async (id, read) =>
    branchNote({ id, readAt: read ? 2 : null })
  );
  commands.setProjectNoteRead.mockImplementation(async (id, read) =>
    projectNote({ id, readAt: read ? 2 : null })
  );
  // A fresh project state store per test.
  vi.resetModules();
  ({ setNoteReadState, writeNoteRead } = await import('./noteReadActions'));
  ({ projectStateStore } = await import('../../stores/projectState.svelte'));
  const { NoteReadTracker } = await import('./noteReadTracker.svelte');
  newTracker = () => new NoteReadTracker();
});

/** Mark a branch note in `projectId` read or unread from a timeline menu. */
function setBranchNote(noteId: string, projectId: string, read: boolean) {
  return setNoteReadState({ kind: 'branch', noteId, projectId }, read);
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
    await setNoteReadState(
      { kind: 'project', noteId: 'project-note-1', projectId: 'stale-project' },
      false
    );

    expect(projectStateStore.isUnread('project-of-note')).toBe(true);
    expect(projectStateStore.isUnread('stale-project')).toBe(false);
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
