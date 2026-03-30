import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, act, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

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
const mockShellOpen = vi.fn().mockResolvedValue(undefined);
vi.mock('@tauri-apps/plugin-shell', () => ({
  open: (...args: unknown[]) => mockShellOpen(...args),
}));

// Mock @tauri-apps/api/window (needed when isDesktopApp is true)
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn().mockResolvedValue(vi.fn()),
    close: vi.fn(),
  }),
}));

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
    getProjectFiles: vi.fn().mockResolvedValue([]),
    getReviews: vi.fn().mockResolvedValue([]),
    clearFocus: vi.fn().mockResolvedValue(undefined),
    checkInstallStatus: vi.fn().mockResolvedValue({ cli: { installed: true }, plugin: { installed: true } }),
  },
}));

// Mock SSE hook
vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

// Must import Layout after mocks are set up
const { default: Layout } = await import('./Layout');

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  document.documentElement.removeAttribute('data-theme');
  vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: false } as MediaQueryList);
});

// E-PENPAL-EXTERNAL-LINKS: verifies handleAppClick intercepts external links when in Tauri/desktop mode.
describe('Layout external link interception (desktop mode)', () => {
  it('intercepts external link clicks and opens via Tauri shell', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    // Wait for initial load
    await waitFor(() => {
      expect(screen.getByTestId('app-layout')).toBeInTheDocument();
    });

    // Add an external link inside the app layout
    const appDiv = screen.getByTestId('app-layout');
    const link = document.createElement('a');
    link.setAttribute('href', 'https://example.com');
    link.textContent = 'External Link';
    appDiv.appendChild(link);

    // Click the external link
    await act(async () => {
      fireEvent.click(link);
    });

    // The Tauri shell open function should have been called with the external URL
    await waitFor(() => {
      expect(mockShellOpen).toHaveBeenCalledWith('https://example.com');
    });

    link.remove();
  });

  it('intercepts http:// links as well', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('app-layout')).toBeInTheDocument();
    });

    const appDiv = screen.getByTestId('app-layout');
    const link = document.createElement('a');
    link.setAttribute('href', 'http://insecure-example.com');
    link.textContent = 'HTTP Link';
    appDiv.appendChild(link);

    await act(async () => {
      fireEvent.click(link);
    });

    await waitFor(() => {
      expect(mockShellOpen).toHaveBeenCalledWith('http://insecure-example.com');
    });

    link.remove();
  });

  it('does not intercept internal links with Tauri shell', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByTestId('app-layout')).toBeInTheDocument();
    });

    const appDiv = screen.getByTestId('app-layout');
    const link = document.createElement('a');
    link.setAttribute('href', '/file/ws1/project-a/notes.md');
    link.textContent = 'Internal Link';
    appDiv.appendChild(link);

    await act(async () => {
      fireEvent.click(link);
    });

    // Shell open should NOT be called for internal links
    expect(mockShellOpen).not.toHaveBeenCalled();

    link.remove();
  });
});
