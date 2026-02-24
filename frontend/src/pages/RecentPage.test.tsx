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
      expect(screen.getByText('plan.md')).toBeTruthy();
      expect(screen.getByText('research.md')).toBeTruthy();
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

  it('shows project path', async () => {
    render(<MemoryRouter><RecentPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('ws/proj/thoughts/plan.md')).toBeTruthy();
    });
  });
});
