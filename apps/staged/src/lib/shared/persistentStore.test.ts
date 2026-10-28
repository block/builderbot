import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

let invokeCommand: ReturnType<typeof vi.fn>;

async function importStore() {
  return await import('./persistentStore');
}

beforeEach(() => {
  vi.resetModules();
  invokeCommand = vi.fn();
  vi.doMock('../transport', () => ({
    isTauri: false,
    invokeCommand,
    listenToEvent: vi.fn(),
  }));
});

afterEach(() => {
  vi.resetAllMocks();
});

describe('initPersistentStore', () => {
  it('shares one preferences_store_path round trip between concurrent callers', async () => {
    let resolvePath!: (path: string) => void;
    invokeCommand.mockReturnValueOnce(
      new Promise<string>((resolve) => {
        resolvePath = resolve;
      })
    );
    const { initPersistentStore, getStoreValue } = await importStore();

    const first = initPersistentStore();
    const second = initPersistentStore();
    expect(invokeCommand).toHaveBeenCalledTimes(1);
    expect(invokeCommand).toHaveBeenCalledWith('preferences_store_path');

    resolvePath('/tmp/preferences.json');
    await Promise.all([first, second]);
    await initPersistentStore();
    expect(invokeCommand).toHaveBeenCalledTimes(1);

    invokeCommand.mockResolvedValueOnce('p1');
    await expect(getStoreValue('last-viewed-project')).resolves.toBe('p1');
  });

  it('lets a later caller retry after the first init fails', async () => {
    invokeCommand.mockRejectedValueOnce(new Error('backend down'));
    const { initPersistentStore } = await importStore();

    await expect(initPersistentStore()).rejects.toThrow('backend down');

    invokeCommand.mockResolvedValueOnce('/tmp/preferences.json');
    await expect(initPersistentStore()).resolves.toBeUndefined();
    expect(invokeCommand).toHaveBeenCalledTimes(2);
  });
});
