import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { vi, describe, it, expect, beforeEach } from 'vitest';
import InReviewPage from './InReviewPage';

vi.mock('../api', () => ({
  api: {
    getInReview: vi.fn().mockResolvedValue([
      {
        workspace: 'ws',
        workspaceURL: '/workspace/ws',
        projectName: 'my-proj',
        projectQN: 'ws/my-proj',
        sourceName: 'thoughts',
        sourceAnchor: 'thoughts',
        badgeText: 'thoughts',
        badgeColor: '#333',
        badgeBg: '#eee',
        agentActive: true,
        typingThreads: 1,
        files: [
          { name: 'plan.md', path: 'thoughts/plan.md', project: 'ws/my-proj', projectPath: '/tmp', openThreads: 3, agentActive: true, fileType: 'plan', age: '1h ago' },
        ],
      },
    ]),
    listProjects: vi.fn().mockResolvedValue([]),
  },
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
}));

vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

describe('InReviewPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders review groups', async () => {
    render(<MemoryRouter><InReviewPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('my-proj')).toBeTruthy();
    });
  });

  it('shows breadcrumb navigation', async () => {
    render(<MemoryRouter><InReviewPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('ws')).toBeTruthy();
      expect(screen.getByText('my-proj')).toBeTruthy();
      expect(screen.getAllByText('thoughts').length).toBeGreaterThan(0);
    });
  });

  it('shows files within groups', async () => {
    render(<MemoryRouter><InReviewPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('thoughts/plan.md')).toBeTruthy();
    });
  });

  it('shows file type badge', async () => {
    render(<MemoryRouter><InReviewPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('plan')).toBeTruthy();
    });
  });

  it('shows title as primary and path as subtitle when title is set', async () => {
    const { api } = await import('../api');
    (api.getInReview as ReturnType<typeof vi.fn>).mockResolvedValueOnce([{
      workspace: 'ws',
      projectName: 'my-proj',
      projectQN: 'ws/my-proj',
      sourceName: 'thoughts',
      sourceAnchor: 'thoughts',
      agentActive: false,
      files: [
        { name: 'plan.md', title: 'My Plan', path: 'thoughts/plan.md', project: 'ws/my-proj', projectPath: '/tmp', openThreads: 1, agentActive: false, fileType: 'plan', age: '1h ago' },
      ],
    }]);
    render(<MemoryRouter><InReviewPage /></MemoryRouter>);
    await waitFor(() => {
      expect(screen.getByText('My Plan')).toBeTruthy();
      expect(screen.getByText('thoughts/plan.md')).toBeTruthy();
    });
  });
});
