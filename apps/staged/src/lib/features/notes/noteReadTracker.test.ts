import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { BranchNote } from '../../types';
import type { NoteReadRow, NoteReadTarget } from './noteReadActions';

const { writeNoteRead, setNoteReadState } = vi.hoisted(() => ({
  writeNoteRead: vi.fn<(kind: string, noteId: string, read: boolean) => Promise<NoteReadRow>>(),
  setNoteReadState: vi.fn<(target: NoteReadTarget, read: boolean) => Promise<NoteReadRow | null>>(),
}));

vi.mock('./noteReadActions', () => ({ writeNoteRead, setNoteReadState }));

type Tracker = import('./noteReadTracker.svelte').NoteReadTracker;

/** The notes as the backend holds them, keyed by id. */
let rows: Map<string, BranchNote>;

function note(id: string, overrides: Partial<BranchNote> = {}): BranchNote {
  return {
    id,
    branchId: 'branch-1',
    sessionId: null,
    title: id,
    content: 'Body',
    createdAt: 1,
    updatedAt: 1,
    completedAt: 10,
    suggestedNextCommitStep: null,
    suggestedNextNoteStep: null,
    parentProjectNoteId: null,
    subtype: null,
    readAt: null,
    ...overrides,
  };
}

/** Mirror `Store::write_read_at`: only a landed note can be marked read. */
function write(noteId: string, read: boolean): BranchNote {
  const row = rows.get(noteId)!;
  const next = {
    ...row,
    readAt: read ? (row.completedAt === null ? row.readAt : Math.max(20, row.completedAt)) : null,
  };
  rows.set(noteId, next);
  return next;
}

async function newTracker(...notes: BranchNote[]): Promise<Tracker> {
  rows = new Map(notes.map((n) => [n.id, n]));
  const { NoteReadTracker } = await import('./noteReadTracker.svelte');
  return new NoteReadTracker();
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  vi.clearAllMocks();
  vi.stubGlobal('$state', (initial: unknown) => initial);
  writeNoteRead.mockImplementation(async (_kind, noteId, read) => write(noteId, read));
  setNoteReadState.mockImplementation(async (target, read) => write(target.noteId, read));
});

describe('NoteReadTracker', () => {
  it('marks a note read when it is shown, once per open', async () => {
    const tracker = await newTracker(note('a'));

    tracker.show('branch', 'a');
    tracker.show('branch', 'a');
    await flush();

    expect(writeNoteRead).toHaveBeenCalledTimes(1);
    expect(writeNoteRead).toHaveBeenCalledWith('branch', 'a', true);
    expect(tracker.canToggle).toBe(true);
    expect(tracker.unread).toBe(false);
  });

  it('keeps a manual unread while the note stays open', async () => {
    const tracker = await newTracker(note('a'));
    tracker.show('branch', 'a');
    await flush();

    await tracker.toggle('project-1');
    expect(setNoteReadState).toHaveBeenCalledWith(
      { kind: 'branch', noteId: 'a', projectId: 'project-1' },
      false
    );
    expect(tracker.unread).toBe(true);

    // The write's notes-changed echo, and the viewer's effect re-running.
    tracker.observe(rows.get('a')!);
    tracker.show('branch', 'a');
    await flush();

    expect(writeNoteRead).toHaveBeenCalledTimes(1);
    expect(rows.get('a')!.readAt).toBeNull();
    expect(tracker.unread).toBe(true);
  });

  it('marks read again once the viewer reopens or navigates', async () => {
    const tracker = await newTracker(note('a'), note('b'));
    tracker.show('branch', 'a');
    await flush();
    await tracker.toggle('project-1');

    tracker.show('branch', 'b');
    tracker.show('branch', 'a');
    await flush();
    expect(rows.get('a')!.readAt).not.toBeNull();

    await tracker.toggle('project-1');
    tracker.reset();
    tracker.show('branch', 'a');
    await flush();
    expect(rows.get('a')!.readAt).not.toBeNull();
    expect(tracker.unread).toBe(false);
  });

  it('marks a note read when it lands while on screen', async () => {
    const tracker = await newTracker(note('a', { completedAt: null }));
    tracker.show('branch', 'a');
    await flush();
    expect(tracker.canToggle).toBe(false);

    rows.set('a', { ...rows.get('a')!, completedAt: 30 });
    tracker.observe(rows.get('a')!);
    await flush();

    expect(writeNoteRead).toHaveBeenCalledTimes(2);
    expect(tracker.canToggle).toBe(true);
    expect(tracker.unread).toBe(false);
  });

  it('marks a revision read when it lands while on screen', async () => {
    const tracker = await newTracker(note('a'));
    tracker.show('branch', 'a');
    await flush();

    // `Store::update_note_title_and_content` clears the read on a revision.
    rows.set('a', { ...rows.get('a')!, content: 'Revised', updatedAt: 40, readAt: null });
    tracker.observe(rows.get('a')!);
    expect(tracker.unread).toBe(true);
    await flush();

    expect(writeNoteRead).toHaveBeenCalledTimes(2);
    expect(rows.get('a')!.readAt).not.toBeNull();
    expect(tracker.unread).toBe(false);
  });

  it('lets a revision override a manual unread, but not a plain refresh', async () => {
    const tracker = await newTracker(note('a'));
    tracker.show('branch', 'a');
    await flush();
    await tracker.toggle('project-1');

    tracker.observe(rows.get('a')!);
    await flush();
    expect(writeNoteRead).toHaveBeenCalledTimes(1);
    expect(tracker.unread).toBe(true);

    rows.set('a', { ...rows.get('a')!, content: 'Revised', updatedAt: 40 });
    tracker.observe(rows.get('a')!);
    await flush();
    expect(writeNoteRead).toHaveBeenCalledTimes(2);
    expect(tracker.unread).toBe(false);
  });

  it('writes nothing when new content leaves the note read', async () => {
    const tracker = await newTracker(note('a', { subtype: 'written' }));
    tracker.show('branch', 'a');
    await flush();

    // The user's own edit to a note they wrote keeps it read.
    rows.set('a', { ...rows.get('a')!, content: 'Edited', updatedAt: 40 });
    tracker.observe(rows.get('a')!);
    await flush();

    expect(writeNoteRead).toHaveBeenCalledTimes(1);
    expect(tracker.unread).toBe(false);
  });

  it('offers no toggle for a child note', async () => {
    const tracker = await newTracker(note('child', { parentProjectNoteId: 'project-note-1' }));

    tracker.show('branch', 'child');
    await flush();

    expect(tracker.canToggle).toBe(false);
    await tracker.toggle('project-1');
    expect(setNoteReadState).not.toHaveBeenCalled();
  });

  it('drops a read that resolves after the viewer moved on', async () => {
    const tracker = await newTracker(note('a'), note('b'));
    let resolveA!: (row: NoteReadRow) => void;
    writeNoteRead.mockImplementationOnce(
      () => new Promise<NoteReadRow>((resolve) => (resolveA = resolve))
    );

    tracker.show('branch', 'a');
    tracker.show('branch', 'b');
    await flush();
    resolveA(write('a', true));
    await flush();

    await tracker.toggle('project-1');
    expect(setNoteReadState).toHaveBeenCalledWith(
      { kind: 'branch', noteId: 'b', projectId: 'project-1' },
      false
    );
  });
});
