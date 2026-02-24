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
      expect(screen.getByText('plan.md')).toBeTruthy();
      expect(screen.getByText('research.md')).toBeTruthy();
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
});
