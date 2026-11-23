// Runs in the "component" vitest project for its svelte plugin, not for a
// DOM: the controller's $derived fields (its rows and the ids Resume sends)
// have to compile for the client, as in the app, to follow the selection.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { InterruptedSession, LaunchRecovery } from '../../types';
import { readReactively } from '../../stores/testStubs/readReactively.svelte';

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
    // A rebase pipeline whose agent turn was cut off resumes like the rest.
    session({ sessionId: 'c', label: 'rebase on fix-login' }),
  ],
  projectSessions: [
    session({ sessionId: 'd', branchId: null, branchName: null }),
    session({
      sessionId: 'e',
      branchId: null,
      branchName: null,
      disposition: 'manual',
      reason: 'the project session never reached its agent',
    }),
    session({ sessionId: 'f', projectId: 'p2', projectName: 'Gadgets', branchId: null }),
    // A project with nothing that can be resumed: listed, never checked.
    session({
      sessionId: 'g',
      projectId: 'p3',
      projectName: 'Gizmos',
      branchId: null,
      disposition: 'manual',
      reason: 'the project session never reached its agent',
    }),
  ],
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

describe('resumeOnLaunch selection', () => {
  it('opens with every project that can be resumed checked', async () => {
    const controller = await importController();
    await controller.load();
    expect(controller.selected).toEqual(['project:p1', 'project:p2']);
    expect(controller.isSelected('project:p3')).toBe(false);
    expect(controller.selectedSessionIds).toEqual(['a', 'b', 'c', 'd', 'f']);
    expect(controller.canResume).toBe(true);
  });

  it('unchecking and rechecking a project changes what Resume sends', async () => {
    const controller = await importController();
    await controller.load();
    controller.setSelected('project:p1', false);
    expect(controller.selectedSessionIds).toEqual(['f']);
    controller.setSelected('project:p1', true);
    expect(controller.selectedSessionIds).toEqual(['a', 'b', 'c', 'd', 'f']);
  });

  it('never checks a project with nothing resumable', async () => {
    const controller = await importController();
    await controller.load();
    controller.setSelected('project:p3', true);
    expect(controller.isSelected('project:p3')).toBe(false);
    controller.setAllSelected(false);
    controller.setAllSelected(true);
    expect(controller.selected).toEqual(['project:p1', 'project:p2']);
  });

  it('disables Resume while nothing is checked', async () => {
    const controller = await importController();
    await controller.load();
    controller.setAllSelected(false);
    expect(controller.selectedSessionIds).toEqual([]);
    expect(controller.canResume).toBe(false);
    await controller.resume();
    expect(resumeInterruptedSessions).not.toHaveBeenCalled();
    expect(controller.pending).toEqual(offer);
  });

  it('groups the offer once and re-filters only when the selection changes', async () => {
    const controller = await importController();
    // Read from an effect, as the dialog's template does, so the derived
    // fields are connected and cached.
    const stop = readReactively(() => [controller.rows, controller.selectedSessionIds]);
    await controller.load();
    const rows = controller.rows;
    expect(rows.map((row) => row.key)).toEqual(['project:p1', 'project:p2', 'project:p3']);
    const ids = controller.selectedSessionIds;
    expect(controller.canResume).toBe(true);
    expect(controller.selectedSessionIds).toBe(ids);

    controller.setSelected('project:p1', false);
    expect(controller.rows).toBe(rows);
    expect(controller.selectedSessionIds).toEqual(['f']);

    await controller.dismiss();
    expect(controller.rows).toEqual([]);
    expect(controller.selectedSessionIds).toEqual([]);
    stop();
  });

  it('disables Resume when no project can be resumed', async () => {
    getInterruptedSessions.mockResolvedValueOnce({
      repoSessions: [],
      projectSessions: [offer.projectSessions[3]],
    });
    const controller = await importController();
    await controller.load();
    expect(controller.selected).toEqual([]);
    expect(controller.canResume).toBe(false);
  });
});

describe('resumeOnLaunch answers', () => {
  it('"Resume" sends every restartable session of the checked projects and closes', async () => {
    const controller = await importController();
    await controller.load();
    await controller.resume();
    expect(resumeInterruptedSessions).toHaveBeenCalledWith(['a', 'b', 'c', 'd', 'f']);
    expect(controller.pending).toBeNull();
    expect(controller.busy).toBe(false);
    expect(controller.selected).toEqual([]);
  });

  it('"Resume" with a project unchecked sends only the rest', async () => {
    const controller = await importController();
    await controller.load();
    controller.setSelected('project:p2', false);
    await controller.resume();
    // One answer either way: the backend records the handled marker, so the
    // unchecked project is left as it is and not offered again.
    expect(resumeInterruptedSessions).toHaveBeenCalledTimes(1);
    expect(resumeInterruptedSessions).toHaveBeenCalledWith(['a', 'b', 'c', 'd']);
    expect(controller.pending).toBeNull();
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
    const first = controller.resume();
    expect(controller.busy).toBe(true);
    expect(controller.canResume).toBe(false);
    // The selection is locked while the answer is in flight.
    controller.setSelected('project:p2', false);
    expect(controller.isSelected('project:p2')).toBe(true);
    await controller.dismiss();
    await controller.resume();
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
    await controller.resume();
    expect(toastError).toHaveBeenCalledWith(
      'Could not resume interrupted sessions: Database not initialized'
    );
    expect(controller.pending).toBeNull();
    errorSpy.mockRestore();
  });
});
