import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes, Outlet } from 'react-router-dom';
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
        worktrees: [
          { name: 'my-project', path: '/tmp/ws/my-project', branch: 'main', isMain: true },
          { name: 'feature-wt', path: '/tmp/ws/my-project/.claude/worktrees/feature-wt', branch: 'feature', isMain: false },
          { name: 'bugfix-wt', path: '/tmp/ws/my-project/.claude/worktrees/bugfix-wt', branch: 'bugfix', isMain: false },
        ],
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
      {
        name: 'single-wt',
        qualifiedName: 'ws/single-wt',
        workspace: 'ws',
        projectPath: '/tmp/ws/single-wt',
        origin: 'workspace',
        badges: [],
        branch: 'develop',
        dirty: true,
        fileCount: 3,
        lastModified: '2026-01-01',
        worktrees: [
          { name: 'single-wt', path: '/tmp/ws/single-wt', branch: 'develop', isMain: true },
          { name: 'one-extra', path: '/tmp/ws/single-wt/.claude/worktrees/one-extra', branch: 'fix', isMain: false },
        ],
      },
    ]),
    getProjectInfo: vi.fn().mockResolvedValue({ fileCount: 5, dirty: false, unpushedCommits: 0 }),
    getInReview: vi.fn().mockResolvedValue([]),
    clearFocus: vi.fn().mockResolvedValue(undefined),
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
    <MemoryRouter initialEntries={['/workspace/ws']}>
      <Routes>
        <Route element={<ContextProvider />}>
          <Route path="/workspace/:name" element={<WorkspacePage />} />
        </Route>
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

  it('shows worktree count excluding main', async () => {
    renderPage();
    await waitFor(() => {
      // my-project has 2 non-main worktrees
      expect(screen.getByText('+ 2 worktrees')).toBeTruthy();
    });
  });

  it('uses singular worktree for count of 1', async () => {
    renderPage();
    await waitFor(() => {
      // single-wt has 1 non-main worktree
      expect(screen.getByText('+ 1 worktree')).toBeTruthy();
    });
  });

  it('does not show worktree count when there are no extra worktrees', async () => {
    renderPage();
    await waitFor(() => {
      expect(screen.getByText('empty-project')).toBeTruthy();
    });
    // empty-project has no worktrees at all
    const card = screen.getByText('empty-project').closest('.project-card');
    expect(card?.querySelector('.worktree-count')).toBeNull();
  });

  it('always shows branch alongside worktree count', async () => {
    renderPage();
    await waitFor(() => {
      const card = screen.getByText('my-project').closest('.project-card');
      const meta = card?.querySelector('.project-card-meta');
      expect(meta?.querySelector('.branch')?.textContent).toBe('main');
      expect(meta?.querySelector('.worktree-count')?.textContent).toBe('+ 2 worktrees');
    });
  });
});
