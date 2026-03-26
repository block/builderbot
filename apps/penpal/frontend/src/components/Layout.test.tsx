import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, act, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
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

// Must import Layout after Highlight polyfill
const { default: Layout } = await import('./Layout');

// Mock API
vi.mock('../api', () => ({
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
  api: {
    listProjects: vi.fn().mockResolvedValue([
      {
        name: 'project-a',
        qualifiedName: 'ws1/project-a',
        workspace: 'ws1',
        origin: 'workspace',
        badges: [],
        fileCount: 5,
        lastModified: '2026-01-01T00:00:00Z',
      },
    ]),
    getInReview: vi.fn().mockResolvedValue([]),
    clearFocus: vi.fn().mockResolvedValue(undefined),
  },
}));

// Mock SSE hook — capture callbacks
vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
  vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: false } as MediaQueryList);
});

describe('Layout', () => {
  it('renders topbar with logo and search', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    expect(screen.getByText('Penpal')).toBeInTheDocument();
    expect(screen.getByPlaceholderText('Search all thoughts...')).toBeInTheDocument();
  });

  it('renders tab bar', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    expect(screen.getByTestId('topbar-tabs')).toBeInTheDocument();
  });

  it('renders initial tab matching URL', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    const tabs = screen.getByTestId('topbar-tabs');
    expect(tabs.querySelector('.tab-bar-tab.active .tab-title')?.textContent).toBe('Recent');
  });

  it('renders new tab button', () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    expect(screen.getByLabelText('New tab')).toBeInTheDocument();
  });

  it('renders sidebar', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    expect(screen.getByTestId('sidebar')).toBeInTheDocument();
    expect(screen.getByText('In Review')).toBeInTheDocument();
  });

  it('renders theme toggle', () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    expect(screen.getByLabelText('Toggle dark mode')).toBeInTheDocument();
  });

  it('renders back and forward buttons', () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    const backBtn = screen.getByLabelText('Go back');
    const fwdBtn = screen.getByLabelText('Go forward');
    expect(backBtn).toBeInTheDocument();
    expect(fwdBtn).toBeInTheDocument();
    // Both should be disabled on a fresh tab
    expect(backBtn).toBeDisabled();
    expect(fwdBtn).toBeDisabled();
  });

  it('renders workspace items after API loads', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    // Wait for projects to load
    expect(await screen.findByText('ws1')).toBeInTheDocument();
  });

  it('refreshes projects and review count on SSE reconnect', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    // Wait for initial load
    await waitFor(() => {
      expect(api.listProjects).toHaveBeenCalledTimes(1);
      expect(api.getInReview).toHaveBeenCalledTimes(1);
    });

    // Get the onReconnect callback (2nd argument to useSSE)
    const useSSEMock = vi.mocked(useSSE);
    const onReconnect = useSSEMock.mock.calls[0]?.[1];
    expect(onReconnect).toBeDefined();

    // Simulate SSE reconnect
    vi.mocked(api.listProjects).mockClear();
    vi.mocked(api.getInReview).mockClear();
    act(() => {
      onReconnect!();
    });

    await waitFor(() => {
      expect(api.listProjects).toHaveBeenCalledTimes(1);
      expect(api.getInReview).toHaveBeenCalledTimes(1);
    });
  });

  it('clears window focus on pagehide', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    act(() => {
      window.dispatchEvent(new Event('pagehide'));
    });

    await waitFor(() => {
      expect(api.clearFocus).toHaveBeenCalledWith({ keepalive: true });
    });
  });

  it('opens a new tab on SSE navigate event', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(api.listProjects).toHaveBeenCalled();
    });

    // Should start with one tab
    const tabBar = screen.getByTestId('topbar-tabs');
    expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(1);

    // Get the onEvent callback (1st argument to useSSE)
    const useSSEMock = vi.mocked(useSSE);
    const onEvent = useSSEMock.mock.calls[0]?.[0];
    expect(onEvent).toBeDefined();

    // Simulate a navigate SSE event for a new path
    act(() => {
      onEvent!({ type: 'navigate', path: '/file/project/doc.md' } as SSEEvent);
    });

    // Should now have two tabs
    await waitFor(() => {
      expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(2);
    });
  });

  it('activates existing tab on SSE navigate event for already-open path', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(api.listProjects).toHaveBeenCalled();
    });

    const tabBar = screen.getByTestId('topbar-tabs');

    // Get the onEvent callback
    const useSSEMock = vi.mocked(useSSE);
    const onEvent = useSSEMock.mock.calls[0]?.[0];

    // Open a second tab via navigate event
    act(() => {
      onEvent!({ type: 'navigate', path: '/file/project/doc.md' } as SSEEvent);
    });

    await waitFor(() => {
      expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(2);
    });

    // Switch back to first tab by clicking it
    act(() => {
      (tabBar.querySelector('.tab-bar-tab') as HTMLElement)?.click();
    });

    // Now navigate to the same path again — should NOT create a third tab
    // We need to get the latest callback since tabs state changed
    const latestOnEvent = useSSEMock.mock.calls[useSSEMock.mock.calls.length - 1]?.[0];
    act(() => {
      latestOnEvent!({ type: 'navigate', path: '/file/project/doc.md' } as SSEEvent);
    });

    // Should still have exactly two tabs
    await waitFor(() => {
      expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(2);
    });
    // The second tab should be active
    const activeTab = tabBar.querySelector('.tab-bar-tab.active .tab-title');
    expect(activeTab?.textContent).toBe('doc.md');
  });

  it('uses client-side navigation for internal link clicks (preserves back button)', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(api.listProjects).toHaveBeenCalled();
    });

    // Back button should be disabled initially
    expect(screen.getByLabelText('Go back')).toBeDisabled();

    // Add a plain <a> tag (simulating a markdown-rendered link) inside the layout
    const appDiv = screen.getByTestId('app-layout');
    const link = document.createElement('a');
    link.setAttribute('href', '/file/ws1/project-a/notes.md');
    link.textContent = 'Notes';
    appDiv.appendChild(link);

    // Click the link — should use client-side navigation, not full page reload
    await act(async () => {
      fireEvent.click(link);
    });

    // Back button should be enabled: client-side navigation preserved tab history
    await waitFor(() => {
      expect(screen.getByLabelText('Go back')).not.toBeDisabled();
    });

    link.remove();
  });
});
