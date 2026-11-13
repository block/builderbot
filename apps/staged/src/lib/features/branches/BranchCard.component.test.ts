import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Branch, BranchTimeline, WorkspaceStatus } from '../../types';
import type { DragDropSubscription } from './dragDrop';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

/** The props the card hands its note dialog, which is stubbed out. */
interface NoteModalProps {
  noteId?: string | null;
  onClose: () => void;
  onStartSessionFromChat?: (
    mode: 'note' | 'commit',
    request: { prompt: string; imageIds: string[] }
  ) => Promise<boolean>;
  startSessionFromChatDisabled?: boolean;
}

const { subscribeDragDrop, toast, cmd, emptyTimeline, noteModal } = vi.hoisted(() => {
  const emptyTimeline: BranchTimeline = {
    commits: [],
    notes: [],
    reviews: [],
    images: [],
    gitState: null,
  };
  return {
    emptyTimeline,
    noteModal: { props: null as NoteModalProps | null },
    subscribeDragDrop: vi.fn((_sub: DragDropSubscription) => () => {}),
    toast: { success: vi.fn(), error: vi.fn() },
    cmd: {
      readTextFile: vi.fn(async (_filePath: string) => '# Dropped\n\nbody'),
      createNote: vi.fn(async (_branchId: string, _title: string, _content: string) => ({})),
      createImage: vi.fn(
        async (_branchId: string | null, _projectId: string, _filePath: string) => ({
          id: 'image-1',
        })
      ),
      createImageFromData: vi.fn(
        async (
          _branchId: string | null,
          _projectId: string,
          _filename: string,
          _mimeType: string,
          _data: string
        ) => ({ id: 'image-2' })
      ),
      getBranchTimeline: vi.fn(
        async (_branchId: string, _options?: { force?: boolean }) => emptyTimeline
      ),
      startOrQueueBranchSession: vi.fn(
        async (
          _branchId: string,
          _prompt: string,
          _sessionType: string,
          _provider?: string,
          _imageIds?: string[]
        ) => ({ sessionId: 'session-new', sessionStatus: 'running' })
      ),
    },
  };
});

vi.mock('./dragDrop', () => ({ subscribeDragDrop }));
vi.mock('svelte-sonner', () => ({ toast }));
// The source module rather than `api/commands`, which only re-exports it.
// The timeline reads resolve to an empty timeline, so a running remote or a
// local card with a worktree loads and shows it.
vi.mock('../../commands', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../commands')>()),
  listProjectNotes: async () => [],
  getBranchTimelineWithRevalidation: () => ({ cached: null, fresh: null }),
  refreshBranchGitState: async () => {},
  ...cmd,
}));

// The card's children are stubbed out: these tests exercise the card root's
// drag-drop subscription and the "Add image" picker. The timeline stub shows
// only the "adding..." rows; the picker stub picks one PNG per button.
vi.mock(
  '../timeline/BranchTimeline.svelte',
  () => import('../sessions/testStubs/BranchTimelineStub.svelte')
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
// The note dialog renders nothing and records the props the card gave it.
vi.mock('../notes/NoteModal.svelte', () => ({
  default: (_anchor: unknown, props: NoteModalProps) => {
    noteModal.props = props;
  },
}));
vi.mock('../notes/WriteNoteModal.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  '../../shared/ConfirmDeleteDialog.svelte',
  () => import('../sessions/testStubs/ConfirmDeleteDialogStub.svelte')
);
vi.mock('./BranchCardHeaderInfo.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./BranchCardActionsBar.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./BranchCardPrButton.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock(
  './BranchImagePicker.svelte',
  () => import('../sessions/testStubs/BranchImagePickerStub.svelte')
);

import BranchCard from './BranchCard.svelte';

const mounted: ReturnType<typeof mount>[] = [];

/** Mounts a card. Set `props.deleting` afterwards to flip its delete state. */
function mountCard(branch: Branch, extra: { worktreeError?: string } = {}) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  const props = reactiveProps({ branch, deleting: false, ...extra });
  mounted.push(mount(BranchCard, { target, props }));
  flushSync();
  return { card: target.querySelector<HTMLElement>('.branch-card')!, props };
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
    deferredPush: null,
    deferredPushRequestedAt: null,
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

/** The card's one drag-drop subscription. */
function subscription() {
  expect(subscribeDragDrop).toHaveBeenCalledTimes(1);
  return subscribeDragDrop.mock.calls[0][0];
}

/** Drops `paths` on the card through its subscription. */
function drop(paths: string[]) {
  subscription().onDrop(paths);
  flushSync();
}

/** Picks one PNG through "Add image": a path on desktop, a `File` on the web. */
function pick(via: 'paths' | 'files') {
  document.querySelector<HTMLButtonElement>(`[data-testid="pick-image-${via}"]`)!.click();
  flushSync();
}

/** Titles of the "adding..." rows on the card's timeline (none without one). */
function pendingRows() {
  flushSync();
  return [...document.querySelectorAll<HTMLElement>('[data-testid="pending-add"]')].map(
    (row) => row.dataset.title
  );
}

/** Waits for a card with a timeline key to load its timeline and show it. */
async function timelineShown() {
  await vi.waitFor(() => {
    flushSync();
    expect(document.querySelector('[data-testid="branch-timeline"]')).not.toBeNull();
  });
}

/**
 * Waits until a drop or pick has fully finished, so the test can then assert
 * what it did not do: no toast, or no second one. `finished` asserts the
 * test's positive signal, such as its stores having resolved, and every
 * "adding..." row the card shows must have cleared too. A row for a file that
 * could not be stored clears as it fails, but one for a stored note or image
 * stays until the reload after the stores is over. The first check runs
 * straight away, before any store can have resolved; the rest run from a
 * timer, so by the time one passes, the microtasks queued behind the signal
 * (counting what was stored, the toast) have run as well.
 */
async function settle(finished: () => void) {
  await vi.waitFor(() => {
    finished();
    expect(pendingRows()).toEqual([]);
  });
}

afterEach(() => {
  for (const card of mounted.splice(0)) unmount(card);
  document.body.innerHTML = '';
  noteModal.props = null;
  subscribeDragDrop.mockClear();
  toast.success.mockClear();
  toast.error.mockClear();
  // Back to the implementations given to vi.fn above.
  for (const fn of Object.values(cmd)) fn.mockReset();
});

describe('BranchCard file drops', () => {
  it.each<WorkspaceStatus>(['running', 'starting', 'stopped', 'suspended', 'error'])(
    'subscribes a remote card whose workspace is %s, with its root as the drop target',
    (status) => {
      const { card } = mountCard(remoteBranch(status));

      const sub = subscription();
      expect(sub.element).toBe(card);
      expect(sub.blocking).toBeFalsy();
    }
  );

  it('highlights a remote card while files are dragged over it', () => {
    const { card } = mountCard(remoteBranch('stopped'));
    const sub = subscription();

    sub.onDragOver(true);
    flushSync();
    expect(card.classList.contains('drag-over')).toBe(true);

    sub.onDragOver(false);
    flushSync();
    expect(card.classList.contains('drag-over')).toBe(false);
  });

  it('stores the files dropped on a stopped remote card against the branch', async () => {
    mountCard(remoteBranch('stopped'));

    drop(['/tmp/a.md', '/tmp/b.png']);
    await settle(() => {
      expect(cmd.createNote).toHaveResolvedTimes(1);
      expect(cmd.createImage).toHaveResolvedTimes(1);
    });

    expect(cmd.readTextFile).toHaveBeenCalledWith('/tmp/a.md');
    expect(cmd.createNote).toHaveBeenCalledWith('branch-1', 'Dropped', 'body');
    expect(cmd.createImage).toHaveBeenCalledWith('branch-1', 'project-1', '/tmp/b.png');
  });

  it('creates the notes in drop order when the files are read out of order', async () => {
    // The backend reads the files concurrently: the big one dropped first
    // answers last.
    const reads = new Map<string, (content: string) => void>();
    cmd.readTextFile.mockImplementation(
      (filePath) => new Promise<string>((resolve) => reads.set(filePath, resolve))
    );
    mountCard(remoteBranch('stopped'));

    drop(['/tmp/big.md', '/tmp/small.md', '/tmp/tiny.md']);
    await vi.waitFor(() => expect(reads.size).toBe(3));
    reads.get('/tmp/tiny.md')!('tiny body');
    reads.get('/tmp/small.md')!('small body');
    // Give a note created as soon as its own read answered the time to be.
    await new Promise((resolve) => setTimeout(resolve, 0));
    reads.get('/tmp/big.md')!('big body');
    await settle(() => expect(cmd.createNote).toHaveResolvedTimes(3));

    expect(cmd.createNote.mock.calls.map(([, title]) => title)).toEqual(['big', 'small', 'tiny']);
  });

  it('stores the rest in drop order, and reports the one file that could not be read', async () => {
    const reads = new Map<
      string,
      { answer: (content: string) => void; fail: (reason: Error) => void }
    >();
    cmd.readTextFile.mockImplementation(
      (filePath) => new Promise<string>((answer, fail) => reads.set(filePath, { answer, fail }))
    );
    mountCard(remoteBranch('running'));
    await timelineShown();
    // Hold the reload after the stores open, so the stored notes keep their
    // "adding..." rows and the failed file's row can be told from them.
    let landReload!: () => void;
    cmd.getBranchTimeline.mockImplementationOnce(
      () => new Promise((resolve) => (landReload = () => resolve(emptyTimeline)))
    );

    drop(['/tmp/first.md', '/tmp/bad.md', '/tmp/third.md', '/tmp/fourth.md']);
    await vi.waitFor(() => expect(reads.size).toBe(4));
    expect(pendingRows()).toEqual(['first', 'bad', 'third', 'fourth']);

    // The reads come back out of order, the failure among them, and the
    // first file's last. Until it is back nothing is created or reported.
    reads.get('/tmp/fourth.md')!.answer('fourth body');
    reads.get('/tmp/bad.md')!.fail(new Error('unreadable'));
    reads.get('/tmp/third.md')!.answer('third body');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(cmd.createNote).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
    expect(pendingRows()).toEqual(['first', 'bad', 'third', 'fourth']);

    reads.get('/tmp/first.md')!.answer('first body');
    await vi.waitFor(() => expect(cmd.getBranchTimeline).toHaveBeenCalledTimes(2));

    // The failed read skipped its own file only.
    expect(cmd.createNote.mock.calls.map(([, title]) => title)).toEqual([
      'first',
      'third',
      'fourth',
    ]);
    expect(cmd.createNote).toHaveResolvedTimes(3);
    expect(toast.error).toHaveBeenCalledTimes(1);
    expect(toast.error).toHaveBeenCalledWith('Error', {
      description: 'Could not read "bad" \u2014 unreadable',
    });
    // Its row is gone as it fails; the stored notes keep theirs for the reload.
    expect(pendingRows()).toEqual(['first', 'third', 'fourth']);

    landReload();
    await settle(() => expect(cmd.getBranchTimeline).toHaveResolvedTimes(2));
    expect(toast.error).toHaveBeenCalledTimes(1);
  });
});

describe('BranchCard while it is being deleted', () => {
  it('takes neither the highlight nor the files dropped on it', async () => {
    const { card, props } = mountCard(remoteBranch('stopped'));
    const sub = subscription();

    props.deleting = true;
    flushSync();
    sub.onDragOver(true);
    flushSync();
    expect(card.classList.contains('drag-over')).toBe(false);
    drop(['/tmp/during-delete.md', '/tmp/during-delete.png']);

    // The delete failed and the card is back. A drop now is stored, and once
    // it has settled, anything the drop above set off would have shown too.
    props.deleting = false;
    flushSync();
    drop(['/tmp/after.md']);
    await settle(() => expect(cmd.createNote).toHaveResolved());

    expect(cmd.readTextFile).toHaveBeenCalledTimes(1);
    expect(cmd.readTextFile).toHaveBeenCalledWith('/tmp/after.md');
    expect(cmd.createImage).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', expect.anything());
  });
});

describe('BranchCard drops the timeline cannot show yet', () => {
  it.each<WorkspaceStatus>(['starting', 'stopped', 'suspended', 'error'])(
    'confirms the stored files once on a remote card whose workspace is %s',
    async (status) => {
      mountCard(remoteBranch(status));

      drop(['/tmp/a.md', '/tmp/b.png']);
      await settle(() => {
        expect(cmd.createNote).toHaveResolvedTimes(1);
        expect(cmd.createImage).toHaveResolvedTimes(1);
      });

      expect(toast.success).toHaveBeenCalledTimes(1);
      expect(toast.success).toHaveBeenCalledWith('Added 2 files', {
        description: 'They will appear on the timeline once the workspace is running.',
      });
    }
  );

  it('reloads the timeline once a stopped remote card that had loaded it runs again', async () => {
    const { props } = mountCard(remoteBranch('running'));
    await timelineShown();
    expect(cmd.getBranchTimeline).toHaveBeenCalledTimes(1);

    props.branch = remoteBranch('stopped');
    flushSync();
    drop(['/tmp/b.png']);
    await settle(() => expect(cmd.createImage).toHaveResolvedTimes(1));
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: 'It will appear on the timeline once the workspace is running.',
    });
    // The store's invalidation is ignored while the workspace is stopped.
    window.dispatchEvent(
      new CustomEvent('timeline-invalidated', { detail: { branchIds: ['branch-1'] } })
    );
    expect(cmd.getBranchTimeline).toHaveBeenCalledTimes(1);

    props.branch = remoteBranch('running');
    flushSync();
    await vi.waitFor(() => expect(cmd.getBranchTimeline).toHaveResolvedTimes(2));
    await timelineShown();
  });

  it('confirms the stored files on a local card with no worktree', async () => {
    mountCard(localBranch(null));

    drop(['/tmp/b.png']);
    await settle(() => expect(cmd.createImage).toHaveResolvedTimes(1));

    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: 'It will appear on the timeline once the worktree is ready.',
    });
  });

  it('confirms the stored files on a local card whose worktree failed', async () => {
    mountCard(localBranch(null), { worktreeError: 'disk full' });

    drop(['/tmp/b.png']);
    await settle(() => expect(cmd.createImage).toHaveResolvedTimes(1));

    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: 'It will appear on the timeline once a retry creates the worktree.',
    });
  });

  it('confirms the stored files on a card showing a failed timeline load', async () => {
    cmd.getBranchTimeline.mockRejectedValue(new Error('git failed'));
    const { card } = mountCard(remoteBranch('running'));
    await vi.waitFor(() => {
      flushSync();
      expect(card.querySelector('.error')?.textContent).toContain('git failed');
    });

    drop(['/tmp/b.png']);
    // The reload after the store fails too, so the card still shows the error.
    await settle(() => {
      expect(cmd.createImage).toHaveResolvedTimes(1);
      expect(cmd.getBranchTimeline).toHaveBeenCalledTimes(2);
    });

    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: 'It will appear once the timeline loads.',
    });
  });

  it('confirms the stored files on a loaded card whose reload fails', async () => {
    mountCard(remoteBranch('running'));
    await timelineShown();
    cmd.getBranchTimeline.mockRejectedValue(new Error('git failed'));

    drop(['/tmp/b.png']);
    // The reload after the store fails, so the card keeps its old timeline.
    await settle(() => {
      expect(cmd.createImage).toHaveResolvedTimes(1);
      expect(cmd.getBranchTimeline).toHaveBeenCalledTimes(2);
    });

    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: 'It will appear once the timeline loads.',
    });
  });

  it('counts only the files that were stored, and still reports the rest', async () => {
    cmd.readTextFile.mockImplementation(async (filePath) => {
      if (filePath === '/tmp/bad.md') throw new Error('unreadable');
      return 'body';
    });
    mountCard(remoteBranch('stopped'));

    drop(['/tmp/a.md', '/tmp/bad.md', '/tmp/b.png']);
    await settle(() => {
      expect(cmd.createNote).toHaveResolvedTimes(1);
      expect(cmd.createImage).toHaveResolvedTimes(1);
      expect(toast.error).toHaveBeenCalledTimes(1);
    });

    expect(toast.error).toHaveBeenCalledWith('Error', {
      description: 'Could not read "bad" — unreadable',
    });
    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 2 files', expect.anything());
  });

  it.each([
    ['a stopped remote card', () => remoteBranch('stopped')],
    ['a local card with no worktree', () => localBranch(null)],
  ])('confirms nothing on %s when no file was stored', async (_label, branch) => {
    cmd.readTextFile.mockRejectedValue(new Error('unreadable'));
    cmd.createImage.mockRejectedValue(new Error('too large'));
    mountCard(branch());

    drop(['/tmp/bad.md', '/tmp/bad.png']);
    // One error toast for the unreadable file, one for the rejected image.
    await settle(() => {
      expect(cmd.createImage).toHaveBeenCalledTimes(1);
      expect(toast.error).toHaveBeenCalledTimes(2);
    });

    expect(toast.success).not.toHaveBeenCalled();
  });

  it.each([
    ['a remote card whose workspace is running', () => remoteBranch('running')],
    ['a local card with a worktree', () => localBranch('/tmp/worktree')],
  ])('leaves %s to its "adding..." rows', async (_label, branch) => {
    mountCard(branch());
    await timelineShown();

    drop(['/tmp/a.md', '/tmp/b.png']);
    expect(pendingRows()).toEqual(['a', 'b.png']);
    await settle(() => {
      expect(cmd.createNote).toHaveResolvedTimes(1);
      expect(cmd.createImage).toHaveResolvedTimes(1);
    });

    expect(toast.success).not.toHaveBeenCalled();
  });

  it('keeps a stored note\'s "adding..." row until the reload after it has landed', async () => {
    mountCard(remoteBranch('running'));
    await timelineShown();
    let landReload!: () => void;
    cmd.getBranchTimeline.mockImplementationOnce(
      () => new Promise((resolve) => (landReload = () => resolve(emptyTimeline)))
    );

    drop(['/tmp/a.md']);
    // The reload after the store, held open.
    await vi.waitFor(() => expect(cmd.getBranchTimeline).toHaveBeenCalledTimes(2));
    expect(cmd.createNote).toHaveResolvedTimes(1);
    expect(pendingRows()).toEqual(['a']);

    landReload();
    await settle(() => expect(cmd.getBranchTimeline).toHaveResolvedTimes(2));
  });
});

describe('BranchCard "Add image" picks the timeline cannot show yet', () => {
  it.each([
    [
      'a remote card whose workspace is starting',
      () => remoteBranch('starting'),
      'the workspace is running',
    ],
    ['a local card with no worktree', () => localBranch(null), 'the worktree is ready'],
  ])('confirms an image picked on %s', async (_label, branch, until) => {
    mountCard(branch());

    pick('paths');
    expect(pendingRows()).toEqual(['picked.png']);
    await settle(() => expect(cmd.createImage).toHaveResolvedTimes(1));

    expect(cmd.createImage).toHaveBeenCalledWith('branch-1', 'project-1', '/tmp/picked.png');
    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: `It will appear on the timeline once ${until}.`,
    });
  });

  it('confirms an image picked through the web file input', async () => {
    mountCard(localBranch(null));

    pick('files');
    await settle(() => expect(cmd.createImageFromData).toHaveResolvedTimes(1));

    expect(cmd.createImageFromData).toHaveBeenCalledWith(
      'branch-1',
      'project-1',
      'picked.png',
      'image/png',
      btoa('png')
    );
    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.success).toHaveBeenCalledWith('Added 1 file', {
      description: 'It will appear on the timeline once the worktree is ready.',
    });
  });

  it('confirms nothing when the picked image is rejected', async () => {
    cmd.createImage.mockRejectedValue(new Error('too large'));
    mountCard(localBranch(null));

    pick('paths');
    await settle(() => {
      expect(cmd.createImage).toHaveBeenCalledTimes(1);
      expect(toast.error).toHaveBeenCalledTimes(1);
    });

    expect(toast.success).not.toHaveBeenCalled();
  });

  it('leaves an image picked on a card with a timeline to its "adding..." row', async () => {
    mountCard(remoteBranch('running'));
    await timelineShown();

    pick('paths');
    expect(pendingRows()).toEqual(['picked.png']);
    await settle(() => expect(cmd.createImage).toHaveResolvedTimes(1));

    expect(toast.success).not.toHaveBeenCalled();
  });
});

describe("BranchCard new session from a note's chat composer", () => {
  const request = { prompt: 'Re: #note:note-1\n\nSplit it up', imageIds: ['image-9'] };

  /** Opens the card's note dialog and returns the props it was given. */
  async function openNote() {
    mountCard(localBranch('/work/tree'));
    await timelineShown();
    document.querySelector<HTMLButtonElement>('[data-testid="open-note"]')!.click();
    flushSync();
    expect(noteModal.props?.noteId).toBe('note-1');
    return noteModal.props!;
  }

  it("starts the chosen kind on the card's branch, as its own buttons do, with the prompt and attachments", async () => {
    const modal = await openNote();
    expect(modal.startSessionFromChatDisabled).toBe(false);

    await expect(modal.onStartSessionFromChat!('commit', request)).resolves.toBe(true);

    expect(cmd.startOrQueueBranchSession).toHaveBeenCalledTimes(1);
    const [branchId, prompt, sessionType, , imageIds] = cmd.startOrQueueBranchSession.mock.calls[0];
    expect({ branchId, prompt, sessionType, imageIds }).toEqual({
      branchId: 'branch-1',
      prompt: request.prompt,
      sessionType: 'commit',
      imageIds: ['image-9'],
    });
    expect(toast.error).not.toHaveBeenCalled();
  });

  it('reports a start that failed, so the composer keeps the draft', async () => {
    const modal = await openNote();
    cmd.startOrQueueBranchSession.mockRejectedValueOnce(new Error('worktree is gone'));

    await expect(modal.onStartSessionFromChat!('note', request)).resolves.toBe(false);

    expect(toast.error).toHaveBeenCalledWith(
      'Unable to start session',
      expect.objectContaining({ description: 'worktree is gone' })
    );
  });

  it('disables the actions while a session start is already in flight', async () => {
    const modal = await openNote();
    let finish!: (response: { sessionId: string; sessionStatus: string }) => void;
    cmd.startOrQueueBranchSession.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        })
    );

    const started = modal.onStartSessionFromChat!('note', request);
    flushSync();
    expect(modal.startSessionFromChatDisabled).toBe(true);

    finish({ sessionId: 'session-new', sessionStatus: 'running' });
    await expect(started).resolves.toBe(true);
    flushSync();
    expect(modal.startSessionFromChatDisabled).toBe(false);
  });
});
