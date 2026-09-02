import { describe, expect, it } from 'vitest';
import { notesChangeAffectsNote } from './noteRefresh';

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
