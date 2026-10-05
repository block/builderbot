import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ListenOptions } from '../../transport';
import {
  DEFAULT_PROJECT_STATUS_OPTIONS,
  type ProjectStatusOption,
} from '../projects/projectStatusDisplay';

const key = 'project-status-options';
const path = '/data/preferences.json';
const custom: ProjectStatusOption = {
  id: 'custom',
  label: 'Testing',
  icon: 'flask-conical',
  color: 'cyan',
};

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

describe.each([true, false])('project status synchronization (native=%s)', (native) => {
  it('shares additions and edits between clients without losing the other client’s status', async () => {
    const a = await newClient(native);
    await a.initPreferences();
    const b = await newClient(!native);
    await b.initPreferences();

    a.setProjectStatusOptions([...a.preferences.projectStatusOptions, custom]);
    expect(b.preferences.projectStatusOptions).toContainEqual(custom);

    const edited = b.preferences.projectStatusOptions.map((option) =>
      option.id === custom.id ? { ...option, label: 'Validated', color: 'green' as const } : option
    );
    b.setProjectStatusOptions(edited);
    expect(a.preferences.projectStatusOptions).toEqual(edited);
    expect(values.get(key)).toEqual(edited);
    // Receiving the event must not write it back and cause an echo loop.
    expect(set).toHaveBeenCalledTimes(2);
  });

  it('respects empty lists and restores defaults on deletion or malformed values', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    emit([]);
    expect(client.preferences.projectStatusOptions).toEqual([]);
    emit([custom]);
    emit(null, { exists: false });
    expect(client.preferences.projectStatusOptions).toEqual(DEFAULT_PROJECT_STATUS_OPTIONS);
    emit([custom]);
    emit('invalid');
    expect(client.preferences.projectStatusOptions).toEqual(DEFAULT_PROJECT_STATUS_OPTIONS);
    expect(set).not.toHaveBeenCalled();
  });

  it('ignores other keys and store files', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    emit([custom], { key: 'unrelated' });
    emit([custom], { path: '/other/preferences.json' });
    expect(client.preferences.projectStatusOptions).toEqual(DEFAULT_PROJECT_STATUS_OPTIONS);
  });

  it('refreshes after registration and reconnection without duplicating listeners', async () => {
    const client = await newClient(native);
    await client.initPreferences();
    await client.initPreferences();
    expect(listeners).toHaveLength(1);

    values.set(key, [custom]);
    listeners[0].options?.onEstablished?.();
    await vi.waitFor(() => expect(client.preferences.projectStatusOptions).toEqual([custom]));

    values.set(key, []);
    listeners[0].options?.onEstablished?.();
    await vi.waitFor(() => expect(client.preferences.projectStatusOptions).toEqual([]));
  });

  it.each(['notification', 'local edit'])(
    'keeps a newer %s over a delayed initial read',
    async (change) => {
      let resolve!: (value: unknown) => void;
      get.mockImplementationOnce(() => new Promise((done) => (resolve = done)));
      const client = await newClient(native);
      const initialized = client.initPreferences();
      await vi.waitFor(() => expect(get).toHaveBeenCalledWith(key));

      if (change === 'notification') emit([custom]);
      else {
        // The local write's event has not arrived yet either.
        set.mockResolvedValueOnce(undefined);
        client.setProjectStatusOptions([custom]);
      }
      resolve([]);
      await initialized;
      expect(client.preferences.projectStatusOptions).toEqual([custom]);
    }
  );

  it('keeps a newer registration snapshot when the initial refresh finishes last', async () => {
    let resolve!: (value: unknown) => void;
    get.mockImplementationOnce(() => new Promise((done) => (resolve = done)));
    const client = await newClient(native);
    const initialized = client.initPreferences();
    await vi.waitFor(() => expect(get).toHaveBeenCalledWith(key));
    values.set(key, [custom]);
    listeners[0].options?.onEstablished?.();
    await vi.waitFor(() => expect(client.preferences.projectStatusOptions).toEqual([custom]));

    resolve([]);
    await initialized;
    expect(client.preferences.projectStatusOptions).toEqual([custom]);
  });

  it('loads an explicitly empty list before enabling the UI', async () => {
    values.set(key, []);
    const client = await newClient(native);
    await client.initPreferences();
    expect(client.preferences.loaded).toBe(true);
    expect(client.preferences.projectStatusOptions).toEqual([]);
  });
});
