import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import RecentPage from './RecentPage';

vi.mock('../api', () => ({
  api: {
    getRecentFiles: vi.fn().mockResolvedValue([
      { name: 'plan.md', path: 'thoughts/plan.md', project: 'ws/proj', age: '1h ago', fileType: 'plan', activityType: 'viewed', activityAge: '5m ago' },
      { name: 'research.md', path: 'thoughts/research.md', project: 'ws/proj', age: '3h ago', fileType: 'research', activityType: 'modified', activityAge: '30m ago' },
    ]),
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

describe('RecentPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders recent files', async () => {
    render(<MemoryRouter><RecentPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('thoughts/plan.md')).toBeTruthy();
      expect(screen.getByText('thoughts/research.md')).toBeTruthy();
    });
  });

  it('shows activity labels', async () => {
    render(<MemoryRouter><RecentPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('viewed 5m ago')).toBeTruthy();
      expect(screen.getByText('modified 30m ago')).toBeTruthy();
    });
  });

  it('shows file type badges', async () => {
    render(<MemoryRouter><RecentPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('plan')).toBeTruthy();
      expect(screen.getByText('research')).toBeTruthy();
    });
  });

  it('shows project as subtitle when no title', async () => {
    render(<MemoryRouter><RecentPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getAllByText('ws/proj').length).toBe(2);
    });
  });

  it('shows title as primary and project/path as subtitle when title is set', async () => {
    const { api } = await import('../api');
    (api.getRecentFiles as ReturnType<typeof vi.fn>).mockResolvedValueOnce([
      { name: 'plan.md', title: 'My Plan', path: 'thoughts/plan.md', project: 'ws/proj', age: '1h ago', fileType: 'plan' },
    ]);
    render(<MemoryRouter><RecentPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('My Plan')).toBeTruthy();
      expect(screen.getByText('ws/proj/thoughts/plan.md')).toBeTruthy();
    });
  });
});
