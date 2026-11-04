import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BranchNote, ProjectNote } from '../../types';

const { commands, markAsUnread, toastError } = vi.hoisted(() => ({
  commands: {
    setNoteRead: vi.fn<(noteId: string, read: boolean) => Promise<BranchNote>>(),
    setProjectNoteRead: vi.fn<(noteId: string, read: boolean) => Promise<ProjectNote>>(),
  },
  markAsUnread: vi.fn<(projectId: string) => void>(),
  toastError: vi.fn(),
}));

vi.mock('../../api/commands', () => commands);
vi.mock('../../stores/projectState.svelte', () => ({ projectStateStore: { markAsUnread } }));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));

import { setNoteReadState, writeNoteRead } from './noteReadActions';

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

beforeEach(() => {
  vi.clearAllMocks();
  commands.setNoteRead.mockImplementation(async (id, read) =>
    branchNote({ id, readAt: read ? 2 : null })
  );
  commands.setProjectNoteRead.mockImplementation(async (id, read) =>
    projectNote({ id, readAt: read ? 2 : null })
  );
});

describe('writeNoteRead', () => {
  it('writes through the command for the note kind', async () => {
    await writeNoteRead('branch', 'note-1', true);
    await writeNoteRead('project', 'project-note-1', false);

    expect(commands.setNoteRead).toHaveBeenCalledWith('note-1', true);
    expect(commands.setProjectNoteRead).toHaveBeenCalledWith('project-note-1', false);
    expect(markAsUnread).not.toHaveBeenCalled();
  });
});

describe('setNoteReadState', () => {
  it('marks a branch note’s project unread along with the note', async () => {
    const row = await setNoteReadState(
      { kind: 'branch', noteId: 'note-1', projectId: 'project-1' },
      false
    );

    expect(row?.readAt).toBeNull();
    expect(markAsUnread).toHaveBeenCalledWith('project-1');
  });

  it('marks the project a project note names unread', async () => {
    await setNoteReadState(
      { kind: 'project', noteId: 'project-note-1', projectId: 'stale-project' },
      false
    );

    expect(markAsUnread).toHaveBeenCalledWith('project-of-note');
  });

  it('leaves the project alone when marking read', async () => {
    const row = await setNoteReadState(
      { kind: 'branch', noteId: 'note-1', projectId: 'project-1' },
      true
    );

    expect(row?.readAt).toBe(2);
    expect(markAsUnread).not.toHaveBeenCalled();
  });

  it('toasts a failed write and leaves the project alone', async () => {
    commands.setNoteRead.mockRejectedValueOnce(new Error('Note not found: note-1'));

    const row = await setNoteReadState(
      { kind: 'branch', noteId: 'note-1', projectId: 'project-1' },
      false
    );

    expect(row).toBeNull();
    expect(toastError).toHaveBeenCalledWith('Unable to mark note as unread', {
      description: 'Note not found: note-1',
    });
    expect(markAsUnread).not.toHaveBeenCalled();
  });
});
