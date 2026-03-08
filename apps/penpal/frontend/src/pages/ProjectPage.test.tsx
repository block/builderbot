import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, Outlet } from 'react-router-dom';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import ProjectPage from './ProjectPage';

vi.mock('../api', () => ({
  api: {
    getProjectFiles: vi.fn().mockResolvedValue([
      {
        name: 'thoughts',
        source: 'thoughts',
        sourceType: 'tree',
        auto: true,
        badgeText: 'thoughts',
        badgeColor: '#333',
        badgeBg: '#eee',
        files: [
          { name: 'plan.md', path: 'thoughts/plan.md', age: '1h ago', fileType: 'plan', source: 'thoughts', sourceType: 'tree' },
          { name: 'research.md', path: 'thoughts/research.md', age: '2h ago', fileType: 'research', source: 'thoughts', sourceType: 'tree' },
        ],
      },
    ]),
    getReviews: vi.fn().mockResolvedValue([]),
    getAgentStatus: vi.fn().mockResolvedValue({ running: false }),
    listProjects: vi.fn().mockResolvedValue([{ qualifiedName: 'ws/proj' }]),
    getInReview: vi.fn().mockResolvedValue([]),
    focusProject: vi.fn().mockResolvedValue(undefined),
  },
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
}));

vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

function ContextProvider() {
  return <Outlet context={{ setHeadings: vi.fn(), setSidebarExtra: vi.fn(), projects: [] }} />;
}

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/project/ws/proj']}>
      <Routes>
        <Route element={<ContextProvider />}>
          <Route path="/project/*" element={<ProjectPage />} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

describe('ProjectPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders source sections', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getAllByText('thoughts').length).toBeGreaterThan(0);
    });
  });

  it('renders file rows within sources', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('plan.md')).toBeTruthy();
      expect(screen.getByText('research.md')).toBeTruthy();
    });
  });

  it('shows file type badges', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('plan')).toBeTruthy();
      expect(screen.getByText('research')).toBeTruthy();
    });
  });

  it('shows file ages', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('1h ago')).toBeTruthy();
    });
  });

  it('shows add source button', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('+ Add to project')).toBeTruthy();
    });
  });

  it('has project-page testid', () => {
    renderPage();
    expect(screen.getByTestId('project-page')).toBeTruthy();
  });

  it('shows title as primary and name as subtitle when title is set', async () => {
    const { api } = await import('../api');
    (api.getProjectFiles as ReturnType<typeof vi.fn>).mockResolvedValueOnce([{
      name: 'thoughts',
      source: 'thoughts',
      sourceType: 'tree',
      auto: true,
      files: [
        { name: 'plan.md', title: 'My Plan', path: 'thoughts/plan.md', age: '1h ago', fileType: 'plan', source: 'thoughts', sourceType: 'tree' },
      ],
    }]);
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('My Plan')).toBeTruthy();
      expect(screen.getByText('plan.md')).toBeTruthy();
    });
  });
});
