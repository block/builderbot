// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

describe('prPollingService in web mode', () => {
  let invokeCommand: ReturnType<typeof vi.fn>;
  let listenToEvent: ReturnType<typeof vi.fn>;
  let unlistenRefresh: ReturnType<typeof vi.fn>;
  let unlistenStale: ReturnType<typeof vi.fn>;
  let randomUUID: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.resetModules();
    invokeCommand = vi.fn().mockResolvedValue(undefined);
    unlistenRefresh = vi.fn();
    unlistenStale = vi.fn();
    listenToEvent = vi.fn().mockReturnValueOnce(unlistenRefresh).mockReturnValueOnce(unlistenStale);
    randomUUID = vi.fn(() => 'web-client-1');

    vi.stubGlobal('crypto', { randomUUID });
    vi.spyOn(document, 'hasFocus').mockReturnValue(true);
    vi.doMock('../transport', () => ({
      isTauri: false,
      invokeCommand,
      listenToEvent,
    }));
  });

  afterEach(() => {
    vi.doUnmock('../transport');
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it('keeps a stable browser client id and sends it with every hint', async () => {
    const service = await import('./prPollingService');
    const clientId = service.getPrPollClientId();

    expect(clientId).toBe('web-client-1');
    expect(service.getPrPollClientId()).toBe(clientId);
    expect(randomUUID).toHaveBeenCalledTimes(1);

    service.init();
    service.setSelectedProject('project-1');
    service.updateChecksStatus('branch-1', 'project-1', true);
    service.refreshNow('project-1');
    window.dispatchEvent(new Event('blur'));
    window.dispatchEvent(new Event('focus'));
    service.dispose();

    expect(listenToEvent.mock.calls.map(([event]) => event)).toEqual([
      'pr-refresh-state',
      'pr-status-stale',
    ]);
    expect(invokeCommand.mock.calls).toEqual([
      ['set_focus', { clientId, focused: true }],
      ['set_foreground_project', { clientId, projectId: 'project-1' }],
      [
        'set_branch_pending',
        { clientId, branchId: 'branch-1', projectId: 'project-1', pending: true },
      ],
      ['refresh_now', { clientId, projectId: 'project-1' }],
      ['set_focus', { clientId, focused: false }],
      ['set_focus', { clientId, focused: true }],
      ['disconnect_client', { clientId }],
    ]);
    expect(unlistenRefresh).toHaveBeenCalledTimes(1);
    expect(unlistenStale).toHaveBeenCalledTimes(1);
  });
});
