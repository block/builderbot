import { describe, expect, it } from 'vitest';
import { liveNoteFor, noteIdentityKey, notesChangeAffectsNote } from './noteRefresh';

const branchWrite = { branchId: 'branch-1', projectId: null };
const projectWrite = { branchId: null, projectId: 'project-1' };
const lagRecovery = { branchId: null, projectId: null };

describe('notesChangeAffectsNote', () => {
  it('matches a branch note against its own branch', () => {
    const target = { noteKind: 'branch' as const, branchId: 'branch-1', projectId: 'project-1' };
    expect(notesChangeAffectsNote(branchWrite, target)).toBe(true);
    expect(notesChangeAffectsNote({ branchId: 'branch-2', projectId: null }, target)).toBe(false);
  });

  it('matches a project note against its own project', () => {
    const target = { noteKind: 'project' as const, projectId: 'project-1' };
    expect(notesChangeAffectsNote(projectWrite, target)).toBe(true);
    expect(notesChangeAffectsNote({ branchId: null, projectId: 'project-2' }, target)).toBe(false);
  });

  it('ignores writes of the other kind', () => {
    expect(
      notesChangeAffectsNote(projectWrite, {
        noteKind: 'branch',
        branchId: 'branch-1',
        projectId: 'project-1',
      })
    ).toBe(false);
    expect(
      notesChangeAffectsNote(branchWrite, { noteKind: 'project', projectId: 'project-1' })
    ).toBe(false);
  });

  it('refetches on the feed lag recovery, whatever the note is', () => {
    expect(notesChangeAffectsNote(lagRecovery, { noteKind: 'branch', branchId: 'branch-1' })).toBe(
      true
    );
    expect(
      notesChangeAffectsNote(lagRecovery, { noteKind: 'project', projectId: 'project-1' })
    ).toBe(true);
  });

  it('refetches on anything when the viewer has no scoping id', () => {
    expect(notesChangeAffectsNote(branchWrite, { noteKind: 'branch' })).toBe(true);
    expect(notesChangeAffectsNote(branchWrite, { noteKind: 'branch', branchId: null })).toBe(true);
    expect(notesChangeAffectsNote(projectWrite, { noteKind: 'project' })).toBe(true);
  });
});

describe('noteIdentityKey', () => {
  it('names a note by its session when it has one, whatever its id', () => {
    const pending = noteIdentityKey({ noteKind: 'branch', sessionId: 'session-1' });
    expect(pending).not.toBeNull();
    // The host learning the id of a note it opened by session is no new note.
    expect(noteIdentityKey({ noteKind: 'branch', sessionId: 'session-1', noteId: 'note-1' })).toBe(
      pending
    );
    expect(noteIdentityKey({ noteKind: 'branch', sessionId: 'session-2' })).not.toBe(pending);
  });

  it('names a session-less note by its id', () => {
    const key = noteIdentityKey({ noteKind: 'branch', noteId: 'note-1' });
    expect(key).not.toBeNull();
    expect(noteIdentityKey({ noteKind: 'branch', noteId: 'note-2' })).not.toBe(key);
    expect(noteIdentityKey({ noteKind: 'branch', sessionId: null, noteId: 'note-1' })).toBe(key);
  });

  it('keeps branch and project notes apart, and a session apart from an id', () => {
    expect(noteIdentityKey({ noteKind: 'project', noteId: 'x' })).not.toBe(
      noteIdentityKey({ noteKind: 'branch', noteId: 'x' })
    );
    expect(noteIdentityKey({ noteKind: 'branch', sessionId: 'x' })).not.toBe(
      noteIdentityKey({ noteKind: 'branch', noteId: 'x' })
    );
  });

  it('is null when nothing names a note', () => {
    expect(noteIdentityKey({ noteKind: 'branch' })).toBeNull();
    expect(noteIdentityKey({ noteKind: 'branch', sessionId: '', noteId: null })).toBeNull();
  });
});

describe('liveNoteFor', () => {
  const identity = 'branch:session:session-1';
  const fetched = { identity, id: 'note-1', title: 'Plan', content: 'v2', updatedAt: 2_000 };

  it('has nothing to offer before a fetch lands', () => {
    expect(liveNoteFor(null, identity, 1_000)).toBeNull();
  });

  it('overrides a snapshot of the same note that is older or the same age', () => {
    expect(liveNoteFor(fetched, identity, 1_000)).toBe(fetched);
    expect(liveNoteFor(fetched, identity, 2_000)).toBe(fetched);
  });

  it('overrides a snapshot that carries no timestamp, as a note not written yet does', () => {
    expect(liveNoteFor(fetched, identity, undefined)).toBe(fetched);
    expect(liveNoteFor(fetched, identity, null)).toBe(fetched);
  });

  it('gives way to a snapshot of the same note written after it', () => {
    expect(liveNoteFor(fetched, identity, 2_001)).toBeNull();
  });

  it('never overrides another note, however old that note is', () => {
    expect(liveNoteFor(fetched, 'branch:session:session-2', 500)).toBeNull();
    expect(liveNoteFor(fetched, null, undefined)).toBeNull();
  });
});
