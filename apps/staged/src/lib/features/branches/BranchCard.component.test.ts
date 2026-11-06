import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Branch, WorkspaceStatus } from '../../types';
import type { DragDropSubscription } from './dragDrop';

const { subscribeDragDrop } = vi.hoisted(() => ({
  subscribeDragDrop: vi.fn((_sub: DragDropSubscription) => () => {}),
}));

vi.mock('./dragDrop', () => ({ subscribeDragDrop }));
// The source module rather than `api/commands`, which only re-exports it.
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  listProjectNotes: async () => [],
}));

// The card's children are stubbed out: these tests exercise only the card
// root's drag-drop subscription.
vi.mock(
  '../timeline/BranchTimeline.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock(
  '../timeline/ImageViewerModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../sessions/SessionModal.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../sessions/NewSessionModal.svelte',
  () => import('../sessions/testStubs/EmptyStub.svelte')
);
vi.mock('../notes/NoteModal.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('../notes/WriteNoteModal.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/ConfirmDeleteDialog.svelte',
  () => import('../sessions/testStubs/ConfirmDeleteDialogStub.svelte')
);
vi.mock('./BranchCardHeaderInfo.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./BranchCardActionsBar.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./BranchCardPrButton.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./BranchImagePicker.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));

import BranchCard from './BranchCard.svelte';

const mounted: ReturnType<typeof mount>[] = [];

function mountCard(branch: Branch) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const card = mount(BranchCard, { target, props: { branch } });
  mounted.push(card);
  flushSync();
  return target;
}

function remoteBranch(workspaceStatus: WorkspaceStatus): Branch {
  return {
    id: 'branch-1',
    projectId: 'project-1',
    projectRepoId: null,
    branchName: 'feature',
    baseBranch: 'main',
    prNumber: null,
    branchType: 'remote',
    workspaceName: 'ws-1',
    workstationId: 1,
    workspaceStatus,
    setupComplete: true,
    worktreePath: null,
    createdAt: 1,
    updatedAt: 1,
    prState: null,
    prChecksStatus: null,
    prReviewDecision: null,
    prMergeable: null,
    prDraft: null,
    prUrl: null,
    prUpdatedAt: null,
    prFetchedAt: null,
    prHeadSha: null,
    prTitle: null,
  };
}

afterEach(() => {
  for (const card of mounted.splice(0)) unmount(card);
  document.body.innerHTML = '';
  subscribeDragDrop.mockClear();
});

describe('BranchCard file drops', () => {
  it.each<WorkspaceStatus>(['running', 'starting', 'stopped', 'suspended', 'error'])(
    'subscribes a remote card whose workspace is %s, with its root as the drop target',
    (status) => {
      const target = mountCard(remoteBranch(status));

      expect(subscribeDragDrop).toHaveBeenCalledTimes(1);
      const [sub] = subscribeDragDrop.mock.calls[0];
      expect(sub.element).toBe(target.querySelector('.branch-card'));
      expect(sub.blocking).toBeFalsy();
    }
  );

  it('highlights a remote card while files are dragged over it', () => {
    const target = mountCard(remoteBranch('stopped'));
    const [sub] = subscribeDragDrop.mock.calls[0];
    const card = target.querySelector('.branch-card')!;

    sub.onDragOver(true);
    flushSync();
    expect(card.classList.contains('drag-over')).toBe(true);

    sub.onDragOver(false);
    flushSync();
    expect(card.classList.contains('drag-over')).toBe(false);
  });
});
