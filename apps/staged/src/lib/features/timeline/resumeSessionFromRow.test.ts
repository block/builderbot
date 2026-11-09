import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { ResumeSessionOutcome } from '../../commands';

const { resumeSession, toastError, toastInfo } = vi.hoisted(() => ({
  resumeSession: vi.fn(
    async (
      _sessionId: string,
      _prompt: string,
      _imageIds?: string[],
      _branchId?: string | null
    ): Promise<{ outcome: 'started' } | { outcome: 'queuedForTransfer'; message: unknown }> => ({
      outcome: 'started',
    })
  ),
  toastError: vi.fn(),
  toastInfo: vi.fn(),
}));

vi.mock('../../api/commands', () => ({ resumeSession }));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError, info: toastInfo } }));

import { resumeSessionFromRow } from './resumeSessionFromRow';

/** Hold the next `resumeSession` call open until the returned release runs. */
function holdNextResume(): { release: () => void; fail: (e: Error) => void } {
  let release!: () => void;
  let fail!: (e: Error) => void;
  resumeSession.mockImplementationOnce(
    () =>
      new Promise<ResumeSessionOutcome>((resolve, reject) => {
        release = () => resolve({ outcome: 'started' });
        fail = reject;
      })
  );
  return { release: () => release(), fail: (e) => fail(e) };
}

beforeEach(() => {
  resumeSession.mockReset();
  resumeSession.mockImplementation(async () => ({ outcome: 'started' }));
  toastError.mockReset();
  toastInfo.mockReset();
  vi.spyOn(console, 'error').mockImplementation(() => {});
});

describe('resumeSessionFromRow', () => {
  it('resumes with the row prompt on the branch the row sits on', async () => {
    await expect(resumeSessionFromRow('session-1', 'branch-1')).resolves.toBe(true);
    expect(resumeSession).toHaveBeenCalledWith(
      'session-1',
      'Continue where you left off.',
      undefined,
      'branch-1'
    );
  });

  it('says the resume is queued when the project is being transferred', async () => {
    resumeSession.mockResolvedValueOnce({ outcome: 'queuedForTransfer', message: {} });
    const refreshed = vi.fn();
    await expect(resumeSessionFromRow('session-1', 'branch-1', refreshed)).resolves.toBe(true);
    expect(toastInfo).toHaveBeenCalledWith('Queued until the transfer completes');
    expect(toastError).not.toHaveBeenCalled();
    // The row re-reads either way: the queued follow-up shows on it.
    expect(refreshed).toHaveBeenCalled();
  });

  it('drops a repeat click while the session is resuming, without a toast', async () => {
    const first = holdNextResume();
    const firstClick = resumeSessionFromRow('session-1', 'branch-1');

    await expect(resumeSessionFromRow('session-1', 'branch-1')).resolves.toBe(false);
    // From a project row too: the guard is keyed by session, not by list.
    await expect(resumeSessionFromRow('session-1', null)).resolves.toBe(false);
    expect(resumeSession).toHaveBeenCalledTimes(1);
    expect(toastError).not.toHaveBeenCalled();

    first.release();
    await expect(firstClick).resolves.toBe(true);

    // Once it lands, the session can be resumed again.
    await expect(resumeSessionFromRow('session-1', 'branch-1')).resolves.toBe(true);
    expect(resumeSession).toHaveBeenCalledTimes(2);
  });

  it('lets other sessions resume while one is in flight', async () => {
    const first = holdNextResume();
    const firstClick = resumeSessionFromRow('session-1', 'branch-1');

    await expect(resumeSessionFromRow('session-2', 'branch-1')).resolves.toBe(true);
    expect(resumeSession).toHaveBeenCalledTimes(2);

    first.release();
    await firstClick;
  });

  it('toasts a failed resume and frees the session for a retry', async () => {
    const first = holdNextResume();
    const firstClick = resumeSessionFromRow('session-1', null);
    first.fail(new Error('agent unavailable'));

    await expect(firstClick).resolves.toBe(false);
    expect(toastError).toHaveBeenCalledWith('Resume failed', {
      description: 'agent unavailable',
    });

    await expect(resumeSessionFromRow('session-1', null)).resolves.toBe(true);
    expect(resumeSession).toHaveBeenCalledTimes(2);
  });

  describe('afterResume', () => {
    /** An `afterResume` held open until `release` runs or `fail` rejects it. */
    function heldRefresh() {
      let release!: () => void;
      let fail!: (e: Error) => void;
      const refresh = vi.fn(
        () =>
          new Promise<void>((resolve, reject) => {
            release = resolve;
            fail = reject;
          })
      );
      return { refresh, release: () => release(), fail: (e: Error) => fail(e) };
    }

    it('keeps the session in flight until the refresh lands', async () => {
      const held = heldRefresh();
      const firstClick = resumeSessionFromRow('session-1', 'branch-1', held.refresh);
      await vi.waitFor(() => expect(held.refresh).toHaveBeenCalledTimes(1));

      // The branch card's button is still up until its timeline reloads; a
      // click now must not reach the backend or toast.
      await expect(resumeSessionFromRow('session-1', 'branch-1', held.refresh)).resolves.toBe(
        false
      );
      expect(resumeSession).toHaveBeenCalledTimes(1);
      expect(held.refresh).toHaveBeenCalledTimes(1);
      expect(toastError).not.toHaveBeenCalled();

      held.release();
      await expect(firstClick).resolves.toBe(true);

      await expect(resumeSessionFromRow('session-1', 'branch-1')).resolves.toBe(true);
      expect(resumeSession).toHaveBeenCalledTimes(2);
    });

    it('runs the refresh only after the session resumed', async () => {
      const first = holdNextResume();
      const refresh = vi.fn();
      const firstClick = resumeSessionFromRow('session-1', null, refresh);
      await Promise.resolve();
      expect(refresh).not.toHaveBeenCalled();

      first.release();
      await expect(firstClick).resolves.toBe(true);
      expect(refresh).toHaveBeenCalledTimes(1);
    });

    it('skips the refresh when the resume fails', async () => {
      resumeSession.mockRejectedValueOnce(new Error('agent unavailable'));
      const refresh = vi.fn();

      await expect(resumeSessionFromRow('session-1', null, refresh)).resolves.toBe(false);
      expect(refresh).not.toHaveBeenCalled();
      expect(toastError).toHaveBeenCalledTimes(1);
    });

    it('logs a failed refresh without a "Resume failed" toast and frees the session', async () => {
      const held = heldRefresh();
      const firstClick = resumeSessionFromRow('session-1', 'branch-1', held.refresh);
      await vi.waitFor(() => expect(held.refresh).toHaveBeenCalledTimes(1));
      held.fail(new Error('timeline read failed'));

      // The session did resume; only the refresh failed.
      await expect(firstClick).resolves.toBe(true);
      expect(toastError).not.toHaveBeenCalled();
      expect(console.error).toHaveBeenCalledWith(
        'Failed to refresh after resuming session:',
        expect.objectContaining({ message: 'timeline read failed' })
      );

      await expect(resumeSessionFromRow('session-1', 'branch-1')).resolves.toBe(true);
      expect(resumeSession).toHaveBeenCalledTimes(2);
    });
  });
});
