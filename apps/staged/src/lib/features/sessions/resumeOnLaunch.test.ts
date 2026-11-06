import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { InterruptedSession, LaunchRecovery } from '../../types';

function session(overrides: Partial<InterruptedSession>): InterruptedSession {
  return {
    sessionId: 's',
    projectId: 'p1',
    branchId: 'b1',
    sessionType: 'commit',
    label: 'commit on fix-login',
    branchName: 'fix-login',
    projectName: 'Widgets',
    disposition: 'resume',
    reason: null,
    interruptedAt: 0,
    ...overrides,
  };
}

const offer: LaunchRecovery = {
  repoSessions: [
    session({ sessionId: 'a' }),
    session({ sessionId: 'b', disposition: 'restart' }),
    session({ sessionId: 'c', disposition: 'manual', reason: 'pipeline' }),
  ],
  projectSessions: [session({ sessionId: 'd', branchId: null, branchName: null })],
};

let getInterruptedSessions: ReturnType<typeof vi.fn>;
let resumeInterruptedSessions: ReturnType<typeof vi.fn>;
let toastError: ReturnType<typeof vi.fn>;

async function importController() {
  const { resumeOnLaunch } = await import('./resumeOnLaunch.svelte');
  return resumeOnLaunch;
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (projectActions.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
  getInterruptedSessions = vi.fn().mockResolvedValue(offer);
  resumeInterruptedSessions = vi.fn().mockResolvedValue(undefined);
  toastError = vi.fn();
  vi.doMock('../../api/commands', () => ({
    getInterruptedSessions,
    resumeInterruptedSessions,
  }));
  vi.doMock('svelte-sonner', () => ({ toast: { error: toastError } }));
});

describe('resumeOnLaunch.load', () => {
  it('opens the dialog when the launch has something to offer', async () => {
    const controller = await importController();
    expect(await controller.load()).toBe(true);
    expect(controller.pending).toEqual(offer);
  });

  it('stays closed when the set was already taken or is empty', async () => {
    getInterruptedSessions.mockResolvedValueOnce(null);
    const controller = await importController();
    expect(await controller.load()).toBe(false);
    expect(controller.pending).toBeNull();

    getInterruptedSessions.mockResolvedValueOnce({ repoSessions: [], projectSessions: [] });
    expect(await controller.load()).toBe(false);
    expect(controller.pending).toBeNull();
  });

  it('never blocks the launch on a failed lookup', async () => {
    getInterruptedSessions.mockRejectedValueOnce(new Error('no store'));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const controller = await importController();
    expect(await controller.load()).toBe(false);
    expect(controller.pending).toBeNull();
    errorSpy.mockRestore();
  });
});

describe('resumeOnLaunch answers', () => {
  it('"Resume all" sends every restartable session and closes', async () => {
    const controller = await importController();
    await controller.load();
    await controller.resumeAll();
    expect(resumeInterruptedSessions).toHaveBeenCalledWith(['a', 'b', 'd']);
    expect(controller.pending).toBeNull();
    expect(controller.busy).toBe(false);
  });

  it('"Dismiss" records the offer as handled with no sessions', async () => {
    const controller = await importController();
    await controller.load();
    await controller.dismiss();
    expect(resumeInterruptedSessions).toHaveBeenCalledWith([]);
    expect(controller.pending).toBeNull();
  });

  it('ignores a second answer while the first is in flight', async () => {
    let settle!: () => void;
    resumeInterruptedSessions.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        settle = resolve;
      })
    );
    const controller = await importController();
    await controller.load();
    const first = controller.resumeAll();
    expect(controller.busy).toBe(true);
    await controller.dismiss();
    expect(resumeInterruptedSessions).toHaveBeenCalledTimes(1);
    settle();
    await first;
    expect(controller.pending).toBeNull();
  });

  it('reports a failed answer and still closes', async () => {
    resumeInterruptedSessions.mockRejectedValueOnce(new Error('Database not initialized'));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const controller = await importController();
    await controller.load();
    await controller.resumeAll();
    expect(toastError).toHaveBeenCalledWith(
      'Could not resume interrupted sessions: Database not initialized'
    );
    expect(controller.pending).toBeNull();
    errorSpy.mockRestore();
  });
});
