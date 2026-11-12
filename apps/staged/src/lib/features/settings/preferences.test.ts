import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ListenOptions } from '../../transport';

/** The synced key the generic sync tests exercise. */
const key = 'composer-send-mode';
const path = '/data/preferences.json';

interface StoreEvent {
  path: string;
  key: string;
  value: unknown;
  exists: boolean;
}

let values: Map<string, unknown>;
let listeners: { callback: (event: StoreEvent) => void; options?: ListenOptions }[];
let get: ReturnType<typeof vi.fn<(key: string) => Promise<unknown>>>;
let set: ReturnType<typeof vi.fn<(key: string, value: unknown) => Promise<void>>>;
let nativeClient: boolean;

function emit(value: unknown, overrides: Partial<StoreEvent> = {}) {
  for (const listener of listeners) {
    listener.callback({ path, key, value, exists: value !== undefined, ...overrides });
  }
}

async function newClient(native: boolean) {
  vi.resetModules();
  nativeClient = native;
  return import('./preferences.svelte');
}

beforeEach(() => {
  vi.stubGlobal('$state', (value: unknown) => value);
  vi.stubGlobal('window', {
    matchMedia: () => ({ matches: false, addEventListener: vi.fn() }),
  });
  vi.stubGlobal('document', { documentElement: { style: { setProperty: vi.fn() } } });
  // initPreferences starts the dark-mode signal, which wants a MutationObserver
  // and a readable root style; neither is part of this module's contract.
  vi.doMock('../../stores/isDark.svelte', () => ({ darkMode: { init: vi.fn() } }));
  values = new Map();
  listeners = [];
  get = vi.fn(async (key: string) => structuredClone(values.get(key)));
  set = vi.fn(async (changedKey: string, value: unknown) => {
    values.set(changedKey, structuredClone(value));
    emit(value, { key: changedKey });
  });
  vi.doMock('../../transport', () => ({
    isTauri: nativeClient,
    invokeCommand: async (command: string, args?: { key: string; value?: unknown }) => {
      if (command === 'preferences_store_path') return path;
      if (command === 'get_preference') return get(args!.key);
      if (command === 'set_preference') return set(args!.key, args!.value);
      throw new Error(`Unexpected command: ${command}`);
    },
    listenToEvent: (
      event: string,
      callback: (event: StoreEvent) => void,
      options?: ListenOptions
    ) => {
      expect(event).toBe('store://change');
      const listener = { callback, options };
      listeners.push(listener);
      return () => {
        listeners = listeners.filter((entry) => entry !== listener);
      };
    },
  }));
  vi.doMock('@tauri-apps/plugin-store', () => ({ load: async () => ({ get, set }) }));
  vi.doMock('../diff/highlighter', () => ({
    SYNTAX_THEMES: ['laserwave'],
    setSyntaxTheme: vi.fn(),
    getTheme: () => undefined,
    isLightTheme: vi.fn(),
    loadAllThemePreviewColors: vi.fn(),
  }));
  vi.doMock('../../components/ui/dialog/dialogWidth.svelte', () => ({
    hydrateDialogWidths: vi.fn(),
  }));
});

afterEach(() => {
  vi.doUnmock('../../transport');
  vi.doUnmock('@tauri-apps/plugin-store');
  vi.doUnmock('../diff/highlighter');
  vi.doUnmock('../../components/ui/dialog/dialogWidth.svelte');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe.each([true, false])('synced preference plumbing (native=%s)', (native) => {
  it('ignores other keys and store files', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    emit('send', { key: 'unrelated' });
    emit('send', { path: '/other/preferences.json' });
    expect(client.preferences.composerSendMode).toBe('queue');
  });

  it('refreshes after registration and reconnection without duplicating listeners', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    await client.initPreferences();
    // One listener per synced key (new-project location first, then composer
    // send mode, then the transfer dialog's uncommitted-changes and
    // ignored-files answers, then agent env vars).
    expect(listeners).toHaveLength(5);

    values.set(key, 'send');
    listeners[1].options?.onEstablished?.();
    await vi.waitFor(() => expect(client.preferences.composerSendMode).toBe('send'));

    values.set(key, 'queue');
    listeners[1].options?.onEstablished?.();
    await vi.waitFor(() => expect(client.preferences.composerSendMode).toBe('queue'));
  });

  it('keeps a newer notification over a delayed initial read', async () => {
    let resolve!: (value: unknown) => void;
    get.mockImplementation(async (changedKey: string) =>
      changedKey === key
        ? new Promise((done) => (resolve = done))
        : structuredClone(values.get(changedKey))
    );
    const client = await newClient(native);
    const initialized = client.initPreferences();
    await vi.waitFor(() => expect(get).toHaveBeenCalledWith(key));

    emit('send');
    resolve('queue');
    await initialized;
    expect(client.preferences.composerSendMode).toBe('send');
  });

  it('keeps a newer registration snapshot when the initial refresh finishes last', async () => {
    const reads: ((value: unknown) => void)[] = [];
    get.mockImplementation(async (changedKey: string) =>
      changedKey === key
        ? new Promise((done) => reads.push(done))
        : structuredClone(values.get(changedKey))
    );
    const client = await newClient(native);
    const initialized = client.initPreferences();
    await vi.waitFor(() => expect(reads).toHaveLength(1));
    listeners[1].options?.onEstablished?.();
    await vi.waitFor(() => expect(reads).toHaveLength(2));
    reads[1]('send');
    await vi.waitFor(() => expect(client.preferences.composerSendMode).toBe('send'));

    reads[0]('queue');
    await initialized;
    expect(client.preferences.composerSendMode).toBe('send');
  });
});

describe.each([true, false])('composer send mode (native=%s)', (native) => {
  const modeKey = 'composer-send-mode';

  it('defaults to queue and persists the chosen mode globally', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.composerSendMode).toBe('queue');

    client.setComposerSendMode('send');
    expect(client.preferences.composerSendMode).toBe('send');
    expect(values.get(modeKey)).toBe('send');

    // A fresh client reads the remembered mode back.
    const reopened = await newClient(native);
    await reopened.initPreferences();
    expect(reopened.preferences.composerSendMode).toBe('send');
  });

  it('falls back to queue for a malformed stored value', async () => {
    values.set(modeKey, 'later');
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.composerSendMode).toBe('queue');
    expect(set).not.toHaveBeenCalledWith(modeKey, expect.anything());
  });

  it('follows a mode picked in another client without echoing it back', async () => {
    const a = await newClient(native);
    await a.initPreferences();
    const b = await newClient(!native);
    await b.initPreferences();

    a.setComposerSendMode('send');
    expect(b.preferences.composerSendMode).toBe('send');
    b.setComposerSendMode('queue');
    expect(a.preferences.composerSendMode).toBe('queue');
    expect(set).toHaveBeenCalledTimes(2);
  });

  it('falls back to queue when another client deletes or corrupts the mode', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    emit('send', { key: modeKey });
    emit(null, { key: modeKey, exists: false });
    expect(client.preferences.composerSendMode).toBe('queue');
    emit('send', { key: modeKey });
    emit('later', { key: modeKey });
    expect(client.preferences.composerSendMode).toBe('queue');
    expect(set).not.toHaveBeenCalled();
  });

  it('refreshes on reconnection to recover a missed change', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    values.set(modeKey, 'send');
    listeners[1].options?.onEstablished?.();
    await vi.waitFor(() => expect(client.preferences.composerSendMode).toBe('send'));
  });

  it('keeps a mode picked while the initial read is in flight', async () => {
    let resolve!: (value: unknown) => void;
    get.mockImplementation(async (changedKey: string) =>
      changedKey === modeKey
        ? new Promise((done) => (resolve = done))
        : structuredClone(values.get(changedKey))
    );
    const client = await newClient(native);
    const initialized = client.initPreferences();
    await vi.waitFor(() => expect(get).toHaveBeenCalledWith(modeKey));

    // The local write's event has not arrived yet either.
    set.mockResolvedValueOnce(undefined);
    client.setComposerSendMode('send');
    resolve('queue');
    await initialized;
    expect(client.preferences.composerSendMode).toBe('send');
  });
});

describe.each([true, false])('new project location (native=%s)', (native) => {
  const locationKey = 'new-project-location';

  it('defaults to local and persists the chosen location globally', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.newProjectLocation).toBe('local');

    client.setNewProjectLocation('remote');
    expect(client.preferences.newProjectLocation).toBe('remote');
    expect(values.get(locationKey)).toBe('remote');

    // A fresh client (an app restart) reads the remembered location back.
    const reopened = await newClient(native);
    await reopened.initPreferences();
    expect(reopened.preferences.newProjectLocation).toBe('remote');
  });

  it('falls back to local for a malformed stored value', async () => {
    values.set(locationKey, 'cloud');
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.newProjectLocation).toBe('local');
    expect(set).not.toHaveBeenCalledWith(locationKey, expect.anything());
  });

  it('follows a location chosen in another client without echoing it back', async () => {
    const a = await newClient(native);
    await a.initPreferences();
    const b = await newClient(!native);
    await b.initPreferences();

    a.setNewProjectLocation('remote');
    expect(b.preferences.newProjectLocation).toBe('remote');
    b.setNewProjectLocation('local');
    expect(a.preferences.newProjectLocation).toBe('local');
    expect(set).toHaveBeenCalledTimes(2);
  });

  it('falls back to local when another client deletes or corrupts it', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    emit('remote', { key: locationKey });
    emit(null, { key: locationKey, exists: false });
    expect(client.preferences.newProjectLocation).toBe('local');
    emit('remote', { key: locationKey });
    emit('cloud', { key: locationKey });
    expect(client.preferences.newProjectLocation).toBe('local');
    expect(set).not.toHaveBeenCalled();
  });

  it('loads before enabling the UI, since forms in the first paint start on it', async () => {
    const reads: ((value: unknown) => void)[] = [];
    get.mockImplementation(async (changedKey: string) =>
      changedKey === locationKey
        ? new Promise((done) => reads.push(done))
        : structuredClone(values.get(changedKey))
    );
    const client = await newClient(native);
    const initialized = client.initPreferences();
    await vi.waitFor(() => expect(reads).toHaveLength(1));
    expect(client.preferences.loaded).toBe(false);

    reads[0]('remote');
    await vi.waitFor(() => expect(client.preferences.loaded).toBe(true));
    expect(client.preferences.newProjectLocation).toBe('remote');
    await initialized;
  });

  it('loads a superseded first read when nothing newer has landed, then takes the newer one', async () => {
    const reads: ((value: unknown) => void)[] = [];
    get.mockImplementation(async (changedKey: string) =>
      changedKey === locationKey
        ? new Promise((done) => reads.push(done))
        : structuredClone(values.get(changedKey))
    );
    const client = await newClient(native);
    const initialized = client.initPreferences();
    await vi.waitFor(() => expect(reads).toHaveLength(1));

    // The subscription is established mid-read and its refresh supersedes the
    // first. The first read is still the freshest value when it lands, so it
    // applies and unblocks the UI without waiting on the second.
    listeners[0].options?.onEstablished?.();
    await vi.waitFor(() => expect(reads).toHaveLength(2));
    reads[0]('remote');
    await initialized;
    expect(client.preferences.loaded).toBe(true);
    expect(client.preferences.newProjectLocation).toBe('remote');

    reads[1]('local');
    await vi.waitFor(() => expect(client.preferences.newProjectLocation).toBe('local'));
  });
});

describe.each([true, false])('transfer uncommitted changes (native=%s)', (native) => {
  const transferKey = 'transfer-uncommitted-changes';

  it('defaults to carrying them and remembers the last answer globally', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.transferUncommittedChanges).toBe(true);

    client.setTransferUncommittedChanges(false);
    expect(client.preferences.transferUncommittedChanges).toBe(false);
    expect(values.get(transferKey)).toBe(false);

    const reopened = await newClient(native);
    await reopened.initPreferences();
    expect(reopened.preferences.transferUncommittedChanges).toBe(false);
  });

  it('follows another client and falls back to true when the value is malformed or gone', async () => {
    const a = await newClient(native);
    await a.initPreferences();
    const b = await newClient(!native);
    await b.initPreferences();

    a.setTransferUncommittedChanges(false);
    expect(b.preferences.transferUncommittedChanges).toBe(false);
    expect(set).toHaveBeenCalledTimes(1);

    emit('yes', { key: transferKey });
    expect(a.preferences.transferUncommittedChanges).toBe(true);
    emit(false, { key: transferKey });
    emit(null, { key: transferKey, exists: false });
    expect(a.preferences.transferUncommittedChanges).toBe(true);
    expect(set).toHaveBeenCalledTimes(1);
  });
});

describe.each([true, false])('transfer ignored files (native=%s)', (native) => {
  const ignoredKey = 'transfer-ignored-files';

  it('defaults to leaving them behind and remembers the last answer globally', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.transferIgnoredFiles).toBe(false);

    client.setTransferIgnoredFiles(true);
    expect(client.preferences.transferIgnoredFiles).toBe(true);
    expect(values.get(ignoredKey)).toBe(true);
    // Separate from the uncommitted-changes answer.
    expect(client.preferences.transferUncommittedChanges).toBe(true);
    expect(values.has('transfer-uncommitted-changes')).toBe(false);

    const reopened = await newClient(native);
    await reopened.initPreferences();
    expect(reopened.preferences.transferIgnoredFiles).toBe(true);
  });

  it('follows another client and falls back to false when the value is malformed or gone', async () => {
    const a = await newClient(native);
    await a.initPreferences();
    const b = await newClient(!native);
    await b.initPreferences();

    a.setTransferIgnoredFiles(true);
    expect(b.preferences.transferIgnoredFiles).toBe(true);
    expect(set).toHaveBeenCalledTimes(1);

    emit('yes', { key: ignoredKey });
    expect(a.preferences.transferIgnoredFiles).toBe(false);
    emit(true, { key: ignoredKey });
    emit(null, { key: ignoredKey, exists: false });
    expect(a.preferences.transferIgnoredFiles).toBe(false);
    expect(set).toHaveBeenCalledTimes(1);
  });
});

describe.each([true, false])('agent environment variables (native=%s)', (native) => {
  const envKey = 'agent-env-vars';
  const token = { name: 'ANTHROPIC_API_KEY', value: 'sk-test', enabled: true };
  const debug = { name: 'DEBUG', value: '1', enabled: false };

  it('defaults to no variables and persists each agent’s list in order', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.agentEnvVars).toEqual({});
    expect(client.getAgentEnvVars('claude')).toEqual([]);

    client.setAgentEnvVars('claude', [debug, token]);
    client.setAgentEnvVars('goose', [token]);
    expect(client.getAgentEnvVars('claude')).toEqual([debug, token]);
    expect(values.get(envKey)).toEqual({ claude: [debug, token], goose: [token] });

    // A fresh client reads the lists back.
    const reopened = await newClient(native);
    await reopened.initPreferences();
    expect(reopened.preferences.agentEnvVars).toEqual({ claude: [debug, token], goose: [token] });
  });

  it('removes an agent’s entry when its list is emptied', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    client.setAgentEnvVars('claude', [token]);
    client.setAgentEnvVars('goose', [token]);
    client.setAgentEnvVars('claude', []);
    expect(client.preferences.agentEnvVars).toEqual({ goose: [token] });
    expect(values.get(envKey)).toEqual({ goose: [token] });
  });

  it('fills in defaults for older entries and drops malformed stored values', async () => {
    values.set(envKey, { claude: [{ name: 'LEGACY' }], codex: 'FOO=bar' });
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.agentEnvVars).toEqual({
      claude: [{ name: 'LEGACY', value: '', enabled: true }],
    });
    emit('invalid', { key: envKey });
    expect(client.preferences.agentEnvVars).toEqual({});
    expect(set).not.toHaveBeenCalled();
  });

  it('follows a change made in another client without echoing it back', async () => {
    const a = await newClient(native);
    await a.initPreferences();
    const b = await newClient(!native);
    await b.initPreferences();

    a.setAgentEnvVars('claude', [token]);
    expect(b.getAgentEnvVars('claude')).toEqual([token]);
    b.setAgentEnvVars('claude', [{ ...token, enabled: false }]);
    expect(a.getAgentEnvVars('claude')).toEqual([{ ...token, enabled: false }]);
    expect(set).toHaveBeenCalledTimes(2);
  });
});
