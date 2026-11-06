import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Branch, BranchTimeline, WorkspaceStatus } from '../../types';
import type { DragDropSubscription } from './dragDrop';

const { subscribeDragDrop, toast, cmd } = vi.hoisted(() => ({
  subscribeDragDrop: vi.fn((_sub: DragDropSubscription) => () => {}),
  toast: { success: vi.fn(), error: vi.fn() },
  cmd: {
    readTextFile: vi.fn(async (_filePath: string) => '# Dropped\n\nbody'),
    createNote: vi.fn(async (_branchId: string, _title: string, _content: string) => ({})),
    createImage: vi.fn(async (_branchId: string | null, _projectId: string, _filePath: string) => ({
      id: 'image-1',
    })),
  },
}));

const emptyTimeline: BranchTimeline = {
  commits: [],
  notes: [],
  reviews: [],
  images: [],
  gitState: null,
};

vi.mock('./dragDrop', () => ({ subscribeDragDrop }));
vi.mock('svelte-sonner', () => ({ toast }));
// The source module rather than `api/commands`, which only re-exports it.
// The timeline reads resolve to an empty timeline, so a running remote or a
// local card with a worktree loads and shows it.
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  listProjectNotes: async () => [],
  getBranchTimelineWithRevalidation: () => ({ cached: null, fresh: null }),
  getBranchTimeline: async () => emptyTimeline,
  refreshBranchGitState: async () => {},
  ...cmd,
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

function localBranch(worktreePath: string | null): Branch {
  return {
    ...remoteBranch('running'),
    branchType: 'local',
    workspaceName: null,
    workstationId: null,
    workspaceStatus: null,
    worktreePath,
  };
}

/**
 * Drops `paths` on the card's subscription and lets the stores, and the
 * reloads that follow them, run out. Every command they reach is mocked to
 * settle on a microtask, so all of it is done by the next macrotask.
 */
async function drop(paths: string[]) {
  const [sub] = subscribeDragDrop.mock.calls[0];
  sub.onDrop(paths);
  await new Promise((resolve) => setTimeout(resolve, 0));
  flushSync();
}

afterEach(() => {
  for (const card of mounted.splice(0)) unmount(card);
  document.body.innerHTML = '';
  subscribeDragDrop.mockClear();
  toast.success.mockClear();
  toast.error.mockClear();
  for (const fn of Object.values(cmd)) fn.mockClear();
  cmd.readTextFile.mockImplementation(async () => '# Dropped\n\nbody');
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

  it('stores the files dropped on a stopped remote card against the branch', async () => {
    mountCard(remoteBranch('stopped'));

    await drop(['/tmp/a.md', '/tmp/b.png']);

    expect(cmd.readTextFile).toHaveBeenCalledWith('/tmp/a.md');
    expect(cmd.createNote).toHaveBeenCalledWith('branch-1', 'Dropped', 'body');
    expect(cmd.createImage).toHaveBeenCalledWith('branch-1', 'project-1', '/tmp/b.png');
  });
});

describe('BranchCard drops the timeline cannot show yet', () => {
  it.each<WorkspaceStatus>(['starting', 'stopped', 'suspended', 'error'])(
    'confirms the stored files on a remote card whose workspace is %s',
    async (status) => {
      mountCard(remoteBranch(status));

      await drop(['/tmp/a.md', '/tmp/b.png']);

      expect(toast.success).toHaveBeenCalledTimes(1);
      expect(toast.success).toHaveBeenCalledWith('Added 2 files', {
        description: 'They will appear on the timeline once the workspace is running.',
      });
    }
  );

  it('confirms the stored files on a local card with no worktree', async () => {
    mountCard(localBranch(null));

    await drop(['/tmp/b.png']);

    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: 'It will appear on the timeline once the worktree is ready.',
    });
  });

  it('counts only the files that were stored, and still reports the rest', async () => {
    cmd.readTextFile.mockImplementation(async (filePath) => {
      if (filePath === '/tmp/bad.md') throw new Error('unreadable');
      return 'body';
    });
    mountCard(remoteBranch('stopped'));

    await drop(['/tmp/a.md', '/tmp/bad.md', '/tmp/b.png']);

    expect(toast.error).toHaveBeenCalledWith('Error', {
      description: 'Could not read "bad" \u2014 unreadable',
    });
    expect(toast.success).toHaveBeenCalledWith('Added 2 files', expect.anything());
  });

  it('confirms nothing when no file was stored', async () => {
    cmd.readTextFile.mockRejectedValue(new Error('unreadable'));
    mountCard(remoteBranch('stopped'));

    await drop(['/tmp/bad.md']);

    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
  });

  it.each([
    ['a remote card whose workspace is running', () => remoteBranch('running')],
    ['a local card with a worktree', () => localBranch('/tmp/worktree')],
  ])('leaves %s to its "adding..." rows', async (_label, branch) => {
    mountCard(branch());

    await drop(['/tmp/a.md', '/tmp/b.png']);

    expect(cmd.createNote).toHaveBeenCalledTimes(1);
    expect(cmd.createImage).toHaveBeenCalledTimes(1);
    expect(toast.success).not.toHaveBeenCalled();
  });
});
