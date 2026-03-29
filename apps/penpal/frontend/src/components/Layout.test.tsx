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
    getProjectFiles: vi.fn().mockResolvedValue([]),
    getReviews: vi.fn().mockResolvedValue([]),
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

// E-PENPAL-HOME-SIDEBAR, E-PENPAL-PROJECT-RESOLVE, E-PENPAL-BREADCRUMB, E-PENPAL-SOURCE-SECTIONS:
// verifies sidebar rendering, tab bar, SSE reconnect, and internal link navigation.
// E-PENPAL-REVIEW-COUNT: verifies review count refresh on SSE events.
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

  it('shows initial tab in tab bar', async () => {
    render(
      <MemoryRouter initialEntries={['/recent']}>
        <Layout />
      </MemoryRouter>,
    );

    const tabs = screen.getByTestId('topbar-tabs');
    // All tabs are shown, including non-file tabs
    expect(tabs.querySelectorAll('.tab-bar-tab')).toHaveLength(1);
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

  // E-PENPAL-FE-HOME-LABEL: "Home" label appears on home screen sidebar
  it('shows "Home" label next to house icon on home screen', async () => {
    render(
      <MemoryRouter initialEntries={['/']}>
        <Layout />
      </MemoryRouter>,
    );

    const sidebar = screen.getByTestId('sidebar');
    const homeLabel = sidebar.querySelector('.home-label');
    expect(homeLabel).toBeInTheDocument();
    expect(homeLabel).toHaveTextContent('Home');
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

    // Should start with one tab (the initial /recent tab)
    const tabBar = screen.getByTestId('topbar-tabs');
    expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(1);

    // Get the onEvent callback (1st argument to useSSE)
    const useSSEMock = vi.mocked(useSSE);
    const onEvent = useSSEMock.mock.calls[0]?.[0];
    expect(onEvent).toBeDefined();

    // Simulate a navigate SSE event for a file path
    act(() => {
      onEvent!({ type: 'navigate', path: '/file/project/doc.md' } as SSEEvent);
    });

    // Should now have two tabs (initial + new file tab)
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
    const useSSEMock = vi.mocked(useSSE);

    // Open a file tab via navigate event
    const onEvent = useSSEMock.mock.calls[0]?.[0];
    act(() => {
      onEvent!({ type: 'navigate', path: '/file/project/doc.md' } as SSEEvent);
    });

    await waitFor(() => {
      expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(2);
    });

    // Navigate to the same path again — should NOT create a third tab
    const latestOnEvent = useSSEMock.mock.calls[useSSEMock.mock.calls.length - 1]?.[0];
    act(() => {
      latestOnEvent!({ type: 'navigate', path: '/file/project/doc.md' } as SSEEvent);
    });

    // Should still have exactly two tabs (reuse existing file tab)
    await waitFor(() => {
      expect(tabBar.querySelectorAll('.tab-bar-tab')).toHaveLength(2);
    });
    const activeTab = tabBar.querySelector('.tab-bar-tab.active .tab-title');
    expect(activeTab?.textContent).toBe('doc.md');
  });

  // E-PENPAL-FE-SRC-DISAMBIG: verifies disambiguation text appears for duplicate badge types.
  it('shows disambiguation path when multiple sources share the same badge', async () => {
    // Mock getProjectFiles to return two ANCHORS groups with same badgeText
    vi.mocked(api.getProjectFiles).mockResolvedValue([
      {
        name: 'apps/auth',
        source: 'anchors',
        sourceType: 'tree',
        auto: true,
        badgeText: 'ANCHORS',
        badgeColor: '#0d9488',
        badgeBg: '#f0fdfa',
        files: [{ name: 'PRODUCT.md', path: 'apps/auth/PRODUCT.md', age: '1h' }],
      },
      {
        name: 'apps/payments',
        source: 'anchors',
        sourceType: 'tree',
        auto: true,
        badgeText: 'ANCHORS',
        badgeColor: '#0d9488',
        badgeBg: '#f0fdfa',
        files: [{ name: 'ERD.md', path: 'apps/payments/ERD.md', age: '2h' }],
      },
      {
        name: 'All Markdown',
        source: '__all_markdown__',
        sourceType: 'tree',
        auto: true,
        files: [],
      },
    ]);

    render(
      <MemoryRouter initialEntries={['/project/ws1/project-a']}>
        <Layout />
      </MemoryRouter>,
    );

    // Wait for project files to load and render
    await waitFor(() => {
      expect(api.getProjectFiles).toHaveBeenCalled();
    });

    // Both disambiguation labels should appear
    const disambigLabels = await screen.findAllByText(/apps\/(auth|payments)/);
    expect(disambigLabels).toHaveLength(2);

    // Verify they have the correct CSS class
    for (const label of disambigLabels) {
      expect(label.className).toBe('source-disambig');
    }
  });

  // E-PENPAL-FE-SRC-DISAMBIG: verifies no disambiguation text when badges are unique.
  it('does not show disambiguation path when source badges are unique', async () => {
    vi.mocked(api.getProjectFiles).mockResolvedValue([
      {
        name: 'thoughts',
        source: 'thoughts',
        sourceType: 'tree',
        auto: true,
        badgeText: 'RPI',
        badgeColor: '#888',
        badgeBg: '#f0f0f0',
        files: [{ name: 'plan.md', path: 'thoughts/plan.md', age: '1h' }],
      },
      {
        name: 'Context',
        source: 'rp1',
        sourceType: 'tree',
        auto: true,
        badgeText: 'RP1',
        badgeColor: '#8b5cf6',
        badgeBg: '#f5f0ff',
        files: [{ name: 'index.md', path: '.rp1/context/index.md', age: '2h' }],
      },
      {
        name: 'All Markdown',
        source: '__all_markdown__',
        sourceType: 'tree',
        auto: true,
        files: [],
      },
    ]);

    render(
      <MemoryRouter initialEntries={['/project/ws1/project-a']}>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(api.getProjectFiles).toHaveBeenCalled();
    });

    // Wait a tick for the state update to propagate
    await waitFor(() => {
      expect(screen.getAllByText('RPI')).toHaveLength(1);
    });

    // No disambiguation labels should exist
    const disambigLabels = document.querySelectorAll('.source-disambig');
    expect(disambigLabels).toHaveLength(0);
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

  // E-PENPAL-SIDEBAR-RESIZE: sidebar resize tests
  describe('sidebar resize', () => {
    it('renders the sidebar resize handle', () => {
      render(
        <MemoryRouter>
          <Layout />
        </MemoryRouter>,
      );

      expect(screen.getByTestId('sidebar-resize-handle')).toBeInTheDocument();
    });

    it('applies default sidebar width to grid template', () => {
      render(
        <MemoryRouter>
          <Layout />
        </MemoryRouter>,
      );

      const app = screen.getByTestId('app-layout');
      expect(app.style.gridTemplateColumns).toBe('240px 4px 1fr');
    });

    it('restores sidebar width from localStorage', () => {
      localStorage.setItem('sidebarWidth', '300');

      render(
        <MemoryRouter>
          <Layout />
        </MemoryRouter>,
      );

      const app = screen.getByTestId('app-layout');
      expect(app.style.gridTemplateColumns).toBe('300px 4px 1fr');
    });

    it('updates sidebar width on drag and persists to localStorage', () => {
      render(
        <MemoryRouter>
          <Layout />
        </MemoryRouter>,
      );

      const handle = screen.getByTestId('sidebar-resize-handle');
      const app = screen.getByTestId('app-layout');

      // Start drag at x=240
      fireEvent.mouseDown(handle, { clientX: 240 });

      // Move to x=340 (delta = +100, new width = 240 + 100 = 340)
      act(() => {
        document.dispatchEvent(new MouseEvent('mousemove', { clientX: 340, buttons: 1 }));
      });

      expect(app.style.gridTemplateColumns).toBe('340px 4px 1fr');

      // Release
      act(() => {
        document.dispatchEvent(new MouseEvent('mouseup'));
      });

      expect(localStorage.getItem('sidebarWidth')).toBe('340');
    });

    it('clamps sidebar width to minimum of 200px', () => {
      render(
        <MemoryRouter>
          <Layout />
        </MemoryRouter>,
      );

      const handle = screen.getByTestId('sidebar-resize-handle');
      const app = screen.getByTestId('app-layout');

      fireEvent.mouseDown(handle, { clientX: 240 });

      // Drag far left (delta = -200, would give 40px, clamped to 200)
      act(() => {
        document.dispatchEvent(new MouseEvent('mousemove', { clientX: 40, buttons: 1 }));
      });

      expect(app.style.gridTemplateColumns).toBe('200px 4px 1fr');

      act(() => {
        document.dispatchEvent(new MouseEvent('mouseup'));
      });
    });

    it('clamps sidebar width to maximum of 700px', () => {
      render(
        <MemoryRouter>
          <Layout />
        </MemoryRouter>,
      );

      const handle = screen.getByTestId('sidebar-resize-handle');
      const app = screen.getByTestId('app-layout');

      fireEvent.mouseDown(handle, { clientX: 240 });

      // Drag far right (delta = +700, would give 940px, clamped to 700)
      act(() => {
        document.dispatchEvent(new MouseEvent('mousemove', { clientX: 940, buttons: 1 }));
      });

      expect(app.style.gridTemplateColumns).toBe('700px 4px 1fr');

      act(() => {
        document.dispatchEvent(new MouseEvent('mouseup'));
      });
    });
  });
});
