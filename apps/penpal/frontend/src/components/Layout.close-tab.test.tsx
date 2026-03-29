import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import type { SSEEvent } from '../types';

// Polyfill CSS Highlight API for test environment (used by FindBar)
if (typeof globalThis.Highlight === 'undefined') {
  globalThis.Highlight = class Highlight {
    clear() {}
    add() {}
    delete() {}
  } as unknown as typeof globalThis.Highlight;
  (globalThis.CSS as Record<string, unknown>).highlights = new Map();
}

// Mock @tauri-apps/plugin-shell
vi.mock('@tauri-apps/plugin-shell', () => ({
  open: vi.fn().mockResolvedValue(undefined),
}));

// E-PENPAL-TABS, E-PENPAL-NEW-WINDOW: stub Tauri internals so the real
// @tauri-apps/api/window module works in the test environment.
// The dynamic import() in handleCloseTab bypasses vi.mock, so we provide
// __TAURI_INTERNALS__ directly and spy on invoke calls.
const mockInvoke = vi.fn().mockResolvedValue(undefined);
beforeEach(() => {
  Object.assign(window, {
    __TAURI_INTERNALS__: {
      metadata: {
        currentWindow: { label: 'main' },
        currentWebview: { label: 'main' },
      },
      invoke: mockInvoke,
      convertFileSrc: vi.fn(),
    },
  });
});
afterEach(() => {
  // @ts-expect-error — cleaning up test-only property
  delete window.__TAURI_INTERNALS__;
});

// Mock API with isDesktopApp = true
vi.mock('../api', () => ({
  API_BASE: 'http://localhost:8080',
  isDesktopApp: true,
  api: {
    listProjects: vi.fn().mockResolvedValue([
      {
        name: 'project-a',
        qualifiedName: 'ws1/project-a',
        workspace: 'ws1',
        origin: 'workspace',
        hasFiles: true,
        lastModified: '2026-01-01T00:00:00Z',
      },
    ]),
    getInReview: vi.fn().mockResolvedValue([]),
    clearFocus: vi.fn().mockResolvedValue(undefined),
    checkInstallStatus: vi.fn().mockResolvedValue({ cli: { installed: true }, plugin: { installed: true } }),
  },
}));

// Mock SSE hook — capture callbacks
const { useSSE } = vi.hoisted(() => ({ useSSE: vi.fn() }));
vi.mock('../hooks/useSSE', () => ({ useSSE }));

// Must import Layout after mocks are set up
const { default: Layout } = await import('./Layout');
const { api } = await import('../api');

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
  vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: false } as MediaQueryList);
});

// E-PENPAL-TABS: verifies that menu-close-tab on the last tab closes the window,
// and with multiple tabs it closes only the active tab.
describe('Layout close tab behavior (desktop mode)', () => {
  it('closes the window when menu-close-tab fires on the last tab', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('topbar-tabs')).toBeInTheDocument();
    });

    // Should have exactly one tab
    const tabBar = screen.getByTestId('topbar-tabs');
    expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(1);

    // Dispatch menu-close-tab — should enter the close-window branch
    await act(async () => {
      window.dispatchEvent(new CustomEvent('menu-close-tab'));
    });

    // clearFocus is called before the window close — proves the last-tab branch was taken
    await waitFor(() => {
      expect(api.clearFocus).toHaveBeenCalled();
    });

    // Tauri's Window.close() calls invoke('plugin:window|close', ...)
    await waitFor(() => {
      const closeCalls = mockInvoke.mock.calls.filter(
        (call: unknown[]) => call[0] === 'plugin:window|close',
      );
      expect(closeCalls).toHaveLength(1);
    });
  });

  it('closes only the active tab when menu-close-tab fires with multiple tabs', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('topbar-tabs')).toBeInTheDocument();
    });

    // Open a second tab via SSE navigate event
    const onEvent = vi.mocked(useSSE).mock.calls[0]?.[0];
    expect(onEvent).toBeDefined();
    act(() => {
      onEvent!({ type: 'navigate', path: '/file/project/doc.md' } as SSEEvent);
    });

    const tabBar = screen.getByTestId('topbar-tabs');
    await waitFor(() => {
      expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(2);
    });

    // Dispatch menu-close-tab — should close the active tab, not the window
    await act(async () => {
      window.dispatchEvent(new CustomEvent('menu-close-tab'));
    });

    await waitFor(() => {
      expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(1);
    });
    // Window close invoke should NOT have been called
    const closeCalls = mockInvoke.mock.calls.filter(
      (call: unknown[]) => call[0] === 'plugin:window|close',
    );
    expect(closeCalls).toHaveLength(0);
  });

  it('hides the close button when only one tab remains', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('topbar-tabs')).toBeInTheDocument();
    });

    const tabBar = screen.getByTestId('topbar-tabs');
    expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(1);

    // No × close button should be visible on the single tab
    expect(tabBar.querySelector('.tab-close')).toBeNull();
  });
});
