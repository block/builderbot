import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import SearchPage from './SearchPage';

vi.mock('../api', () => ({
  api: {
    search: vi.fn().mockResolvedValue({
      query: 'plan',
      matchingProjects: [
        { project: 'planning', qualifiedName: 'ws/planning', projectPath: '/tmp/ws/planning' },
      ],
      projectResults: [
        {
          project: 'my-proj',
          qualifiedName: 'ws/my-proj',
          projectPath: '/tmp/ws/my-proj',
          files: [
            { path: 'thoughts/plan.md', name: 'plan.md', nameMatch: true, fileType: 'plan' },
            { path: 'thoughts/research.md', name: 'research.md', fileType: 'research' },
          ],
        },
      ],
      totalFiles: 2,
    }),
    getInReview: vi.fn().mockResolvedValue([]),
    listProjects: vi.fn().mockResolvedValue([]),
    clearFocus: vi.fn().mockResolvedValue(undefined),
  },
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
}));

vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

function renderWithQuery(q: string) {
  return render(
    <MemoryRouter initialEntries={[`/search?q=${encodeURIComponent(q)}`]}>
      <Routes>
        <Route path="/search" element={<SearchPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('SearchPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('shows matching projects', async () => {
    renderWithQuery('plan');
    await waitFor(() => {
      expect(screen.getByText('planning')).toBeTruthy();
    });
  });

  it('shows file results grouped by project', async () => {
    renderWithQuery('plan');
    await waitFor(() => {
      expect(screen.getByText('my-proj')).toBeTruthy();
      expect(screen.getByText('thoughts/plan.md')).toBeTruthy();
      expect(screen.getByText('thoughts/research.md')).toBeTruthy();
    });
  });

  it('shows name match badge', async () => {
    renderWithQuery('plan');
    await waitFor(() => {
      expect(screen.getByText('name')).toBeTruthy();
    });
  });

  it('shows stats', async () => {
    renderWithQuery('plan');
    await waitFor(() => {
      expect(screen.getByText(/1 project/)).toBeTruthy();
      expect(screen.getByText(/2 files/)).toBeTruthy();
    });
  });

  it('shows title as primary and path as subtitle when title is set', async () => {
    const { api } = await import('../api');
    (api.search as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
      query: 'plan',
      matchingProjects: [],
      projectResults: [{
        project: 'my-proj',
        qualifiedName: 'ws/my-proj',
        projectPath: '/tmp/ws/my-proj',
        files: [
          { path: 'thoughts/plan.md', name: 'plan.md', title: 'My Plan', fileType: 'plan' },
        ],
      }],
      totalFiles: 1,
    });
    renderWithQuery('plan');
    await waitFor(() => {
      expect(screen.getByText('My Plan')).toBeTruthy();
      expect(screen.getByText('thoughts/plan.md')).toBeTruthy();
    });
  });
});
