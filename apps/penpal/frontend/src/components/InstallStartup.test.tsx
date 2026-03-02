import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { api } from '../api';

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

// Mock as desktop app so install logic runs
vi.mock('../api', () => ({
  API_BASE: 'http://localhost:8080',
  isDesktopApp: true,
  api: {
    listProjects: vi.fn().mockResolvedValue([]),
    getInReview: vi.fn().mockResolvedValue([]),
    checkInstallStatus: vi.fn().mockResolvedValue({
      cli: { installed: false },
      plugin: { installed: false },
    }),
    installTools: vi.fn(),
  },
}));

vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: false } as MediaQueryList);
});

describe('Install startup behavior', () => {
  it('shows install modal on first launch when no tools installed', async () => {
    vi.mocked(api.checkInstallStatus).mockResolvedValue({
      cli: { installed: false },
      plugin: { installed: false },
    });

    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByText('Install Command Line Tools')).toBeInTheDocument();
    });
    // Should show "Install" (not "Update") since no tools exist
    expect(screen.getByRole('button', { name: 'Install' })).toBeInTheDocument();
  });

  it('shows update modal when outdated tools are installed', async () => {
    vi.mocked(api.checkInstallStatus).mockResolvedValue({
      cli: { installed: true, path: '/usr/local/bin/penpal' },
      plugin: { installed: true },
    });

    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByText('Update Command Line Tools')).toBeInTheDocument();
    });
    // Should show "Update" since tools already exist
    expect(screen.getByRole('button', { name: 'Update' })).toBeInTheDocument();
  });

  it('does not show modal when no tools installed and previously dismissed', async () => {
    localStorage.setItem(`penpal-install-dismissed-${__BUILD_ID__}`, '1');
    vi.mocked(api.checkInstallStatus).mockResolvedValue({
      cli: { installed: false },
      plugin: { installed: false },
    });

    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    // Wait for status check to complete
    await waitFor(() => {
      expect(api.checkInstallStatus).toHaveBeenCalled();
    });

    // Modal should NOT be visible
    const overlay = document.querySelector('.modal-overlay.open');
    expect(overlay).toBeNull();
  });

  it('does not persist dismiss when user closes without tools installed', async () => {
    vi.mocked(api.checkInstallStatus).mockResolvedValue({
      cli: { installed: false },
      plugin: { installed: false },
    });

    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByText('Not Now')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Not Now'));

    expect(localStorage.getItem(`penpal-install-dismissed-${__BUILD_ID__}`)).toBeNull();
  });

  it('does not persist dismiss when user closes with outdated tools installed', async () => {
    vi.mocked(api.checkInstallStatus).mockResolvedValue({
      cli: { installed: true, path: '/usr/local/bin/penpal' },
      plugin: { installed: true },
    });

    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByText('Not Now')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Not Now'));

    expect(localStorage.getItem(`penpal-install-dismissed-${__BUILD_ID__}`)).toBeNull();
  });

  it('persists dismiss after successful install', async () => {
    vi.mocked(api.checkInstallStatus).mockResolvedValue({
      cli: { installed: true, path: '/usr/local/bin/penpal' },
      plugin: { installed: true },
    });
    vi.mocked(api.installTools).mockResolvedValue({
      cli: { installed: true, path: '/usr/local/bin/penpal' },
      plugin: { installed: true },
    });

    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    await waitFor(() => {
      expect(screen.getByText('Update')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Update'));

    await waitFor(() => {
      expect(screen.getByText('Done')).toBeInTheDocument();
    });

    fireEvent.click(screen.getByText('Done'));

    expect(localStorage.getItem(`penpal-install-dismissed-${__BUILD_ID__}`)).toBe('1');
  });
});
