import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import Layout from './Layout';

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
  },
}));

// Mock SSE hook (no-op)
vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

beforeEach(() => {
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

  it('renders workspace items after API loads', async () => {
    render(
      <MemoryRouter>
        <Layout />
      </MemoryRouter>,
    );

    // Wait for projects to load
    expect(await screen.findByText('ws1')).toBeInTheDocument();
  });
});
