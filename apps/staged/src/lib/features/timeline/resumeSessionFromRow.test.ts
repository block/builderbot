import { beforeEach, describe, expect, it, vi } from 'vitest';

const { resumeSession, toastError } = vi.hoisted(() => ({
  resumeSession: vi.fn(
    async (
      _sessionId: string,
      _prompt: string,
      _imageIds?: string[],
      _branchId?: string | null
    ): Promise<void> => {}
  ),
  toastError: vi.fn(),
}));

vi.mock('../../api/commands', () => ({ resumeSession }));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));

import { resumeSessionFromRow } from './resumeSessionFromRow';

/** Hold the next `resumeSession` call open until the returned release runs. */
function holdNextResume(): { release: () => void; fail: (e: Error) => void } {
  let release!: () => void;
  let fail!: (e: Error) => void;
  resumeSession.mockImplementationOnce(
    () =>
      new Promise<void>((resolve, reject) => {
        release = resolve;
        fail = reject;
      })
  );
  return { release: () => release(), fail: (e) => fail(e) };
}

beforeEach(() => {
  resumeSession.mockReset();
  resumeSession.mockImplementation(async () => {});
  toastError.mockReset();
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
});
