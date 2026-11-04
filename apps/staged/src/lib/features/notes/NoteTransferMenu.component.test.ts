import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

interface DialogProps {
  onPrepare?: () => Promise<void>;
  onConfirm: (targetProjectId: string) => Promise<void>;
}

const { dialog, commands } = vi.hoisted(() => ({
  dialog: { props: null as DialogProps | null },
  commands: {
    getNote: vi.fn(),
    getProjectNote: vi.fn(),
    moveNoteToProject: vi.fn(),
    moveProjectNoteToProject: vi.fn(),
    copyNoteToProject: vi.fn(),
    copyProjectNoteToProject: vi.fn(),
  },
}));

vi.mock('$lib/components/ui/dropdown-menu', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
// The picker is replaced by a capture of the callbacks the menu hands it.
vi.mock('./NoteTransferDialog.svelte', () => ({
  default: (_anchor: unknown, props: DialogProps) => {
    dialog.props = props;
  },
}));
vi.mock('svelte-sonner', () => ({ toast: { success: vi.fn() } }));
vi.mock('../../stores/projectsData.svelte', () => ({ projectsDataStore: { projects: [] } }));
vi.mock('../../api/commands', () => commands);

import NoteTransferMenu from './NoteTransferMenu.svelte';

const settled = { title: 'Plan', sessionId: 'session', completedAt: 1 };

let component: ReturnType<typeof mount> | null = null;

function mountMenu(noteKind: 'branch' | 'project', beforeTransfer = vi.fn(async () => {})) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(NoteTransferMenu, {
    target,
    props: { noteId: 'note-1', noteKind, noteTitle: 'Plan', projectId: 'here', beforeTransfer },
  });
  flushSync();
  return { picker: dialog.props!, beforeTransfer };
}

describe('NoteTransferMenu pre-transfer checks', () => {
  beforeEach(() => {
    dialog.props = null;
    for (const command of Object.values(commands)) command.mockReset();
    commands.getNote.mockResolvedValue(settled);
    commands.getProjectNote.mockResolvedValue(settled);
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('checks the stored note, then saves, before the picker creates anything', async () => {
    const { picker, beforeTransfer } = mountMenu('branch');

    await picker.onPrepare!();
    expect(commands.getNote).toHaveBeenCalledWith('note-1');
    expect(beforeTransfer).toHaveBeenCalledOnce();

    // The save already ran; the transfer itself only runs the command.
    await picker.onConfirm('there');
    expect(commands.moveNoteToProject).toHaveBeenCalledWith('note-1', 'there');
    expect(beforeTransfer).toHaveBeenCalledOnce();
  });

  it('refuses a note its session is still writing, without saving it', async () => {
    commands.getNote.mockResolvedValue({ ...settled, completedAt: null });
    const { picker, beforeTransfer } = mountMenu('branch');

    await expect(picker.onPrepare!()).rejects.toThrow(
      '"Plan" is still being generated; wait for its session to finish before moving it.'
    );
    expect(beforeTransfer).not.toHaveBeenCalled();
  });

  it('refuses a note that was deleted meanwhile', async () => {
    commands.getNote.mockResolvedValue(null);
    const { picker, beforeTransfer } = mountMenu('branch');

    await expect(picker.onPrepare!()).rejects.toThrow('This note no longer exists.');
    expect(beforeTransfer).not.toHaveBeenCalled();
  });

  it('reads a project note from the project notes', async () => {
    commands.getProjectNote.mockResolvedValue({ ...settled, completedAt: null });
    const { picker } = mountMenu('project');

    await expect(picker.onPrepare!()).rejects.toThrow('still being generated');
    expect(commands.getProjectNote).toHaveBeenCalledWith('note-1');
    expect(commands.getNote).not.toHaveBeenCalled();
  });
});
