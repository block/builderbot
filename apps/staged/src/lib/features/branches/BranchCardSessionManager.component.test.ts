// Runs in the "component" vitest project: the manager is a .svelte.ts class
// whose $state/$derived fields need the svelte plugin to compile.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Branch } from '../../types';

const { writer, loadDraftMock } = vi.hoisted(() => ({
  writer: {
    scheduleSave: vi.fn(),
    saveNow: vi.fn(async () => {}),
    clearNow: vi.fn(async () => {}),
    flush: vi.fn(async () => {}),
  },
  loadDraftMock: vi.fn(async (_key: string): Promise<string | null> => null),
}));

vi.mock('../../shared/draftCache', () => ({
  createDraftWriteScheduler: () => writer,
  loadDraft: (key: string) => loadDraftMock(key),
}));
vi.mock('./branchSessionLaunch.svelte', () => ({
  getPendingSessionItems: () => [],
  hasPendingQueuedSession: () => false,
  hasPendingSessionStart: () => false,
  startOrQueueBranchSessionWithPending: vi.fn(async () => {}),
}));

import BranchCardSessionManager from './BranchCardSessionManager.svelte';

const branch = { id: 'branch-1', projectId: 'project-1' } as unknown as Branch;

function createManager() {
  return new BranchCardSessionManager({
    getBranch: () => branch,
    getIsRemote: () => false,
    loadTimeline: () => {},
    getTimeline: () => null,
  });
}

beforeEach(() => {
  writer.scheduleSave.mockClear();
  writer.saveNow.mockClear();
  writer.flush.mockClear();
  loadDraftMock.mockReset();
  loadDraftMock.mockImplementation(async () => null);
});

describe('BranchCardSessionManager referring open', () => {
  it("does not leak another mode's held draft into a commit open racing the referring load", async () => {
    // Regression: openNewSessionReferring used to set newSessionMode='commit'
    // before its awaited flush+read, so a concurrent openNewSession('commit')
    // saw mode === newSessionMode and kept a draftPrompt that actually
    // belonged to the held mode, seeding commit mode with note's text.
    const mgr = createManager();
    // A note draft held in memory from a previous close.
    mgr.handleNewSessionClose({ prompt: 'note text', mode: 'note', imageIds: [] });

    // The referring open's store read stays in flight…
    let resolveCommitLoad!: (value: string | null) => void;
    loadDraftMock.mockImplementation(
      () =>
        new Promise<string | null>((resolve) => {
          resolveCommitLoad = resolve;
        })
    );
    const referringOpen = mgr.openNewSessionReferring('#note:abc123');

    // …while the user clicks "New commit". The commit modal must open with a
    // clean composer, not the held note text.
    mgr.openNewSession('commit');
    expect(mgr.showNewSession).toBe(true);
    expect(mgr.newSessionMode).toBe('commit');
    expect(mgr.draftPrompt).toBe('');

    // The superseded referring open must not touch the prompt when it lands.
    // (Its store read only starts once the writer flush ahead of it settles.)
    await vi.waitFor(() => expect(loadDraftMock).toHaveBeenCalled());
    resolveCommitLoad('stored commit draft');
    await referringOpen;
    expect(mgr.draftPrompt).toBe('');
    expect(mgr.newSessionMode).toBe('commit');
  });

  it('still builds the referring prompt from the stored commit draft when unraced', async () => {
    const mgr = createManager();
    mgr.handleNewSessionClose({ prompt: 'note text', mode: 'note', imageIds: ['note-img'] });

    loadDraftMock.mockImplementation(async () => 'stored commit draft');
    await mgr.openNewSessionReferring('#note:abc123');

    expect(mgr.showNewSession).toBe(true);
    expect(mgr.newSessionMode).toBe('commit');
    expect(mgr.draftPrompt).toContain('stored commit draft');
    expect(mgr.draftPrompt).toContain('#note:abc123');
    expect(mgr.draftPrompt).not.toContain('note text');
    // Images aren't persisted, but the held ones belong to note mode and must
    // not seed the commit modal.
    expect(mgr.draftImageIds).toEqual([]);
  });

  it('keeps held commit-mode images across a referring open', async () => {
    const mgr = createManager();
    mgr.handleNewSessionClose({ prompt: 'commit text', mode: 'commit', imageIds: ['commit-img'] });

    await mgr.openNewSessionReferring('#note:abc123');

    expect(mgr.newSessionMode).toBe('commit');
    expect(mgr.draftPrompt).toContain('commit text');
    expect(mgr.draftImageIds).toEqual(['commit-img']);
  });
});

describe('BranchCardSessionManager per-mode image drafts', () => {
  it("clears another mode's held images when opening a different mode", () => {
    // Regression: openNewSession cleared draftPrompt when opening a different
    // mode but left draftImageIds, so images attached while drafting a note
    // seeded the commit modal via initialImageIds.
    const mgr = createManager();
    mgr.handleNewSessionClose({ prompt: 'note text', mode: 'note', imageIds: ['note-img'] });

    mgr.openNewSession('commit');

    expect(mgr.showNewSession).toBe(true);
    expect(mgr.newSessionMode).toBe('commit');
    expect(mgr.draftPrompt).toBe('');
    expect(mgr.draftImageIds).toEqual([]);
  });

  it('keeps held images when reopening the same mode', () => {
    const mgr = createManager();
    mgr.handleNewSessionClose({ prompt: 'note text', mode: 'note', imageIds: ['note-img'] });

    mgr.openNewSession('note');

    expect(mgr.draftPrompt).toBe('note text');
    expect(mgr.draftImageIds).toEqual(['note-img']);
  });
});
