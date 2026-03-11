import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import CommentsPanel from './CommentsPanel';
import type { ThreadResponse } from '../types';

// Mock the API
vi.mock('../api', () => ({
  api: {
    createThread: vi.fn().mockResolvedValue({}),
    replyToThread: vi.fn().mockResolvedValue({}),
    patchThread: vi.fn().mockResolvedValue({}),
    startAgent: vi.fn().mockResolvedValue({}),
    stopAgent: vi.fn().mockResolvedValue({}),
  },
}));

const mockThread: ThreadResponse = {
  id: 'thread-1',
  status: 'open',
  anchor: {
    selectedText: 'Test selection',
    before: 'before text',
    after: 'after text',
    startLine: 5,
  },
  comments: [
    {
      id: 'comment-1',
      author: 'Alice',
      role: 'human',
      body: 'This needs updating',
      createdAt: '2026-01-01T00:00:00Z',
    },
    {
      id: 'comment-2',
      author: 'Claude',
      role: 'agent',
      body: 'I agree, will update',
      createdAt: '2026-01-01T01:00:00Z',
      suggestedReplies: ['Looks good', 'Please revise'],
      inReplyTo: 'comment-1',
    },
  ],
  createdAt: '2026-01-01T00:00:00Z',
};

const resolvedThread: ThreadResponse = {
  ...mockThread,
  id: 'thread-2',
  status: 'resolved',
  resolvedAt: '2026-01-02T00:00:00Z',
  resolvedBy: 'Alice',
};

function renderPanel(threads: ThreadResponse[] = [mockThread], anchorLines: Record<string, number> = { 'thread-1': 5 }) {
  return render(
    <MemoryRouter>
      <CommentsPanel
        threads={threads}
        anchorLines={anchorLines}
        project="test/project"
        filePath="thoughts/test.md"
        onRefresh={vi.fn()}
      />
    </MemoryRouter>,
  );
}

describe('CommentsPanel', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('shows "No comments yet" when empty', () => {
    renderPanel([]);
    expect(screen.getByText('No comments yet')).toBeDefined();
  });

  it('renders thread anchor text', () => {
    renderPanel();
    expect(screen.getByText(/Test selection/)).toBeDefined();
  });

  it('renders comment author and body', () => {
    renderPanel();
    expect(screen.getByText('Alice')).toBeDefined();
    expect(screen.getByText('Claude')).toBeDefined();
  });

  it('renders role badges', () => {
    const { container } = renderPanel();
    expect(container.querySelector('.comment-role.human')).toBeDefined();
    expect(container.querySelector('.comment-role.agent')).toBeDefined();
  });

  it('renders thread status', () => {
    const { container } = renderPanel();
    const status = container.querySelector('.thread-status.open');
    expect(status).toBeDefined();
    expect(status?.textContent).toBe('open');
  });

  it('renders suggested reply pills for agent comments', () => {
    renderPanel();
    expect(screen.getByText('Looks good')).toBeDefined();
    expect(screen.getByText('Please revise')).toBeDefined();
  });

  it('renders open thread count', () => {
    renderPanel();
    expect(screen.getByText('1')).toBeDefined();
  });

  it('shows review banner for open threads', () => {
    renderPanel();
    expect(screen.getByText(/In review/)).toBeDefined();
    expect(screen.getByText(/1 open thread/)).toBeDefined();
  });

  it('shows reply and resolve buttons', () => {
    renderPanel();
    expect(screen.getByText('Reply')).toBeDefined();
    expect(screen.getByText('Resolve')).toBeDefined();
  });

  it('shows resolved threads toggle when resolved threads exist', () => {
    renderPanel([mockThread, resolvedThread], { 'thread-1': 5, 'thread-2': 10 });
    expect(screen.getByText('1 resolved')).toBeDefined();
  });

  it('shows orphaned warning for anchors not found', () => {
    renderPanel([mockThread], { 'thread-1': -1 });
    expect(screen.getByText('Anchor text not found in document')).toBeDefined();
  });

  it('renders agent working indicator visible when active', () => {
    const workingThread: ThreadResponse = { ...mockThread, agentWorking: true };
    const { container } = renderPanel([workingThread]);
    const el = container.querySelector('.thread-working');
    expect(el).toBeDefined();
    expect(el?.classList.contains('hidden')).toBe(false);
  });

  it('renders agent working indicator hidden when inactive', () => {
    const { container } = renderPanel([mockThread]);
    const el = container.querySelector('.thread-working');
    expect(el).toBeDefined();
    expect(el?.classList.contains('hidden')).toBe(true);
  });

  it('always renders agent indicator in DOM for layout stability', () => {
    const { container } = renderPanel();
    const el = container.querySelector('.agent-indicator');
    expect(el).toBeDefined();
    expect(el?.classList.contains('hidden')).toBe(true);
  });

  it('renders agent status indicator when running', () => {
    render(
      <MemoryRouter>
        <CommentsPanel
          threads={[mockThread]}
          anchorLines={{ 'thread-1': 5 }}
          project="test/project"
          filePath="thoughts/test.md"
          onRefresh={vi.fn()}
          agentStatus={{ running: true, contextPercent: 45 }}
        />
      </MemoryRouter>,
    );
    expect(screen.getByText('Agent')).toBeDefined();
    expect(screen.getByText('45%')).toBeDefined();
  });

  it('agent indicator is visible when running, hidden when not', () => {
    const { container, rerender } = render(
      <MemoryRouter>
        <CommentsPanel
          threads={[mockThread]}
          anchorLines={{ 'thread-1': 5 }}
          project="test/project"
          filePath="thoughts/test.md"
          onRefresh={vi.fn()}
          agentStatus={{ running: true, contextPercent: 45 }}
        />
      </MemoryRouter>,
    );
    const indicator = container.querySelector('.agent-indicator');
    expect(indicator?.classList.contains('hidden')).toBe(false);

    rerender(
      <MemoryRouter>
        <CommentsPanel
          threads={[mockThread]}
          anchorLines={{ 'thread-1': 5 }}
          project="test/project"
          filePath="thoughts/test.md"
          onRefresh={vi.fn()}
          agentStatus={{ running: false }}
        />
      </MemoryRouter>,
    );
    const indicatorAfter = container.querySelector('.agent-indicator');
    expect(indicatorAfter?.classList.contains('hidden')).toBe(true);
  });
});
