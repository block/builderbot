import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

describe('browser-native command wrappers', () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.doUnmock('./transport');
    vi.unstubAllGlobals();
  });

  it('opens URLs with browser navigation in web mode', async () => {
    const opened = { opener: {} } as Window;
    const open = vi.fn(() => opened);
    const assign = vi.fn();
    vi.stubGlobal('window', { open, location: { assign } });

    const { openUrl } = await import('./commands');

    await openUrl('https://example.com/pull/1');

    expect(open).toHaveBeenCalledWith('https://example.com/pull/1', '_blank');
    expect(opened.opener).toBeNull();
    expect(assign).not.toHaveBeenCalled();
  });

  it('falls back to current-tab navigation when a new window cannot be opened', async () => {
    const open = vi.fn(() => null);
    const assign = vi.fn();
    vi.stubGlobal('window', { open, location: { assign } });

    const { openUrl } = await import('./commands');

    await openUrl('https://example.com/pull/2');

    expect(open).toHaveBeenCalledWith('https://example.com/pull/2', '_blank');
    expect(assign).toHaveBeenCalledWith('https://example.com/pull/2');
  });

  it('keeps path-based image uploads desktop-only in web mode', async () => {
    const fetch = vi.fn();
    vi.stubGlobal('fetch', fetch);

    const { createImage } = await import('./commands');

    await expect(createImage('branch-1', 'project-1', '/tmp/image.png')).rejects.toThrow(
      'desktop file paths'
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it('keeps opener discovery desktop-only in web mode', async () => {
    const fetch = vi.fn();
    vi.stubGlobal('window', {});
    vi.stubGlobal('fetch', fetch);

    const { getAvailableOpeners, openInApp } = await import('./features/branches/branch');

    await expect(getAvailableOpeners()).resolves.toEqual([]);
    await expect(openInApp('/tmp/repo', 'finder')).rejects.toThrow('web mode');
    expect(fetch).not.toHaveBeenCalled();
  });

  it('builds note follow-up prompts through the backend command', async () => {
    const invokeCommand = vi.fn().mockResolvedValue('backend prompt');
    vi.doMock('./transport', () => ({
      invokeCommand,
      isTauri: true,
    }));

    const { buildNoteFollowupMessage } = await import('./commands');

    await expect(buildNoteFollowupMessage('session-1', 'branch-1', true)).resolves.toBe(
      'backend prompt'
    );
    expect(invokeCommand).toHaveBeenCalledWith('build_note_followup_message', {
      sessionId: 'session-1',
      branchId: 'branch-1',
      hasParsedNote: true,
    });
  });
});
