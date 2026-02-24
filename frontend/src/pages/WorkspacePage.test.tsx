import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import WorkspacePage from './WorkspacePage';

vi.mock('../api', () => ({
  api: {
    listProjects: vi.fn().mockResolvedValue([
      {
        name: 'my-project',
        qualifiedName: 'ws/my-project',
        workspace: 'ws',
        projectPath: '/tmp/ws/my-project',
        origin: 'workspace',
        badges: [{ text: 'thoughts', color: '#333', bg: '#eee' }],
        branch: 'main',
        dirty: false,
        fileCount: 5,
        lastModified: '2026-01-01',
        agentConnected: false,
        age: '2h ago',
        reviewCount: 2,
      },
      {
        name: 'empty-project',
        qualifiedName: 'ws/empty-project',
        workspace: 'ws',
        projectPath: '/tmp/ws/empty-project',
        origin: 'workspace',
        badges: [],
        fileCount: 0,
        lastModified: '2026-01-01',
      },
    ]),
    getProjectInfo: vi.fn().mockResolvedValue({ fileCount: 5, dirty: false, unpushedCommits: 0 }),
    getInReview: vi.fn().mockResolvedValue([]),
  },
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
}));

vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/workspace/ws']}>
      <Routes>
        <Route path="/workspace/:name" element={<WorkspacePage />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe('WorkspacePage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders project cards', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('my-project')).toBeTruthy();
    });
  });

  it('shows project badges', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('thoughts')).toBeTruthy();
    });
  });

  it('shows branch info', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('main')).toBeTruthy();
    });
  });

  it('shows review count', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('2 in review')).toBeTruthy();
    });
  });

  it('shows deemphasized class for empty projects', async () => {
    renderPage();
    await waitFor(() => {
      const card = screen.getByText('empty-project').closest('.project-card');
      expect(card?.classList.contains('deemphasized')).toBe(true);
    });
  });

  it('has workspace-page testid', () => {
    renderPage();
    expect(screen.getByTestId('workspace-page')).toBeTruthy();
  });
});
