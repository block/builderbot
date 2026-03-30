import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, waitFor, act } from '@testing-library/react';
import { MemoryRouter, Routes, Route, Outlet, useNavigate } from 'react-router-dom';
import FilePage from './FilePage';
import { api } from '../api';
import { useSSE } from '../hooks/useSSE';
import type { LayoutContext } from '../components/Layout';

// Mock the API
vi.mock('../api', () => ({
  API_BASE: 'http://localhost:8080',
  isDesktopApp: false,
  api: {
    getRawFile: vi.fn().mockResolvedValue('# Hello'),
    getThreads: vi.fn().mockResolvedValue([]),
    getAgentStatus: vi.fn().mockResolvedValue({ running: false, project: 'ws/proj' }),
    getProjectFiles: vi.fn().mockResolvedValue([]),
    listProjects: vi.fn().mockResolvedValue([]),
    startAgent: vi.fn().mockResolvedValue({ running: true, project: 'ws/proj' }),
    getPublishState: vi.fn().mockResolvedValue({}),
    recordView: vi.fn().mockResolvedValue(undefined),
    focusProject: vi.fn().mockResolvedValue(undefined),
    focusFile: vi.fn().mockResolvedValue(undefined),
  },
}));

// Mock SSE hook — capture callbacks
vi.mock('../hooks/useSSE', () => ({
  useSSE: vi.fn(),
}));

// Mock MermaidRenderer to avoid DOM complexity
vi.mock('../components/MermaidRenderer', () => ({
  renderMermaidBlocks: vi.fn(),
}));

function LayoutWrapper() {
  const ctx: LayoutContext = {
    setHeadings: vi.fn(),
    projects: [{ name: 'proj', qualifiedName: 'ws/proj', workspace: 'ws', origin: 'workspace' as const, hasFiles: true, lastModified: '', projectPath: '/tmp/proj' }],
  };
  return <Outlet context={ctx} />;
}

function renderFilePage(url = '/file/ws/proj/thoughts/plan.md') {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route element={<LayoutWrapper />}>
          <Route path="/file/*" element={<FilePage />} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

// Helper component to expose programmatic navigation for tests
let testNavigate: ReturnType<typeof useNavigate>;
function NavTrigger() {
  testNavigate = useNavigate();
  return null;
}

function renderFilePageNavigable(url = '/file/ws/proj/thoughts/plan.md') {
  return render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route element={<LayoutWrapper />}>
          <Route path="/file/*" element={<><NavTrigger /><FilePage /></>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

const agentNotRunning = {
  running: false,
  project: 'ws/proj',
  pid: 0,
  startedAt: '',
  contextWindow: 0,
  contextUsed: 0,
  contextPercent: 0,
  totalCostUSD: 0,
  numTurns: 0,
};

const agentRunning = {
  running: true,
  project: 'ws/proj',
  pid: 123,
  startedAt: '2026-01-01T00:00:00Z',
  contextWindow: 200000,
  contextUsed: 1000,
  contextPercent: 0.5,
  totalCostUSD: 0,
  numTurns: 1,
};

// Helper to trigger the SSE event callback captured by the useSSE mock
function getSSECallbacks() {
  const useSSEMock = vi.mocked(useSSE);
  const lastCall = useSSEMock.mock.calls[useSSEMock.mock.calls.length - 1];
  return {
    onEvent: lastCall?.[0] as ((event: { type: string; project?: string }) => void) | undefined,
    onReconnect: lastCall?.[1] as (() => void) | undefined,
  };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('FilePage', () => {
  it('auto-starts agent when needsAgent is true and re-fetches status', async () => {
    vi.mocked(api.getAgentStatus)
      .mockResolvedValueOnce({ ...agentNotRunning, needsAgent: true })
      .mockResolvedValue(agentRunning);

    renderFilePage();

    await waitFor(() => {
      expect(api.startAgent).toHaveBeenCalledWith('ws/proj');
    });

    // After starting, it should re-fetch status to pick up running state
    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalledTimes(2);
    });
  });

  it('starts polling after auto-start so the running dot stays live', async () => {
    vi.useFakeTimers();

    vi.mocked(api.getAgentStatus)
      .mockResolvedValueOnce({ ...agentNotRunning, needsAgent: true })
      .mockResolvedValue(agentRunning);

    renderFilePage();

    // Wait for auto-start flow to complete (fetch → start → re-fetch)
    // Need to flush microtasks since fake timers are active
    await vi.waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalledTimes(2);
    });

    // Advance past the 5s polling interval
    const callsBefore = vi.mocked(api.getAgentStatus).mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });

    // Polling should have fired another status check
    expect(vi.mocked(api.getAgentStatus).mock.calls.length).toBeGreaterThan(callsBefore);

    vi.useRealTimers();
  });

  it('does not auto-start agent when needsAgent is false', async () => {
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);

    renderFilePage();

    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalled();
    });

    expect(api.startAgent).not.toHaveBeenCalled();
  });

  it('does not auto-start agent when already running', async () => {
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentRunning);

    renderFilePage();

    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalled();
    });

    expect(api.startAgent).not.toHaveBeenCalled();
  });

  it('shows pending highlight when comment is started via selection toolbar', async () => {
    const markdown = '# Hello\n\nSome test content here.\n';
    vi.mocked(api.getRawFile).mockResolvedValue(markdown);
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);

    const { container } = renderFilePage();

    await waitFor(() => {
      expect(container.querySelector('#content')).toBeTruthy();
      expect(container.textContent).toContain('Some test content here.');
    });

    expect(container.querySelector('mark.pending-highlight')).toBeNull();

    // Trigger the comment flow via selection + toolbar click.
    // jsdom doesn't support Range.getBoundingClientRect, so mock it.
    const origGetBCR = Range.prototype.getBoundingClientRect;
    Range.prototype.getBoundingClientRect = () => ({ top: 10, bottom: 20, left: 5, right: 50, width: 45, height: 10, x: 5, y: 10, toJSON: () => ({}) });

    const contentEl = container.querySelector('#content')!;
    const walker = document.createTreeWalker(contentEl, NodeFilter.SHOW_TEXT);
    let textNode: Text | null = null;
    while (walker.nextNode()) {
      if ((walker.currentNode as Text).nodeValue?.includes('test content')) {
        textNode = walker.currentNode as Text;
        break;
      }
    }
    expect(textNode).toBeTruthy();

    const range = document.createRange();
    const idx = textNode!.nodeValue!.indexOf('test content');
    range.setStart(textNode!, idx);
    range.setEnd(textNode!, idx + 'test content'.length);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);

    await act(async () => {
      contentEl.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
      await new Promise((r) => setTimeout(r, 20));
    });

    const commentBtn = container.querySelector('.selection-toolbar button');
    expect(commentBtn).toBeTruthy();
    await act(async () => {
      commentBtn!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    await waitFor(() => {
      const mark = container.querySelector('mark.pending-highlight');
      expect(mark).toBeTruthy();
      expect(mark!.textContent).toBe('test content');
      expect(mark!.classList.contains('comment-highlight')).toBe(true);
    });

    Range.prototype.getBoundingClientRect = origGetBCR;
  });

  it('pending highlight disappears when new thread form is cancelled', async () => {
    const markdown = '# Hello\n\nSome test content here.\n';
    vi.mocked(api.getRawFile).mockResolvedValue(markdown);
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);

    const origGetBCR = Range.prototype.getBoundingClientRect;
    Range.prototype.getBoundingClientRect = () => ({ top: 10, bottom: 20, left: 5, right: 50, width: 45, height: 10, x: 5, y: 10, toJSON: () => ({}) });

    const { container } = renderFilePage();

    await waitFor(() => {
      expect(container.textContent).toContain('Some test content here.');
    });

    const contentEl = container.querySelector('#content')!;
    const walker = document.createTreeWalker(contentEl, NodeFilter.SHOW_TEXT);
    let textNode: Text | null = null;
    while (walker.nextNode()) {
      if ((walker.currentNode as Text).nodeValue?.includes('test content')) {
        textNode = walker.currentNode as Text;
        break;
      }
    }
    const range = document.createRange();
    const idx = textNode!.nodeValue!.indexOf('test content');
    range.setStart(textNode!, idx);
    range.setEnd(textNode!, idx + 'test content'.length);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);

    await act(async () => {
      contentEl.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
      await new Promise((r) => setTimeout(r, 20));
    });

    await act(async () => {
      container.querySelector('.selection-toolbar button')!.dispatchEvent(
        new MouseEvent('click', { bubbles: true }),
      );
    });

    await waitFor(() => {
      expect(container.querySelector('mark.pending-highlight')).toBeTruthy();
    });

    const cancelBtn = container.querySelector('.btn-cancel-form');
    expect(cancelBtn).toBeTruthy();
    await act(async () => {
      cancelBtn!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    await waitFor(() => {
      expect(container.querySelector('mark.pending-highlight')).toBeNull();
    });

    Range.prototype.getBoundingClientRect = origGetBCR;
  });

  it('does not crash when SSE fires thread update while pending highlight is active', async () => {
    const markdown = '# Hello\n\nSome test content here.\n';
    vi.mocked(api.getRawFile).mockResolvedValue(markdown);
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);

    const origGetBCR = Range.prototype.getBoundingClientRect;
    Range.prototype.getBoundingClientRect = () => ({ top: 10, bottom: 20, left: 5, right: 50, width: 45, height: 10, x: 5, y: 10, toJSON: () => ({}) });

    const { container } = renderFilePage();

    await waitFor(() => {
      expect(container.textContent).toContain('Some test content here.');
    });

    const contentEl = container.querySelector('#content')!;
    const walker = document.createTreeWalker(contentEl, NodeFilter.SHOW_TEXT);
    let textNode: Text | null = null;
    while (walker.nextNode()) {
      if ((walker.currentNode as Text).nodeValue?.includes('test content')) {
        textNode = walker.currentNode as Text;
        break;
      }
    }
    const range = document.createRange();
    const idx = textNode!.nodeValue!.indexOf('test content');
    range.setStart(textNode!, idx);
    range.setEnd(textNode!, idx + 'test content'.length);
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);

    await act(async () => {
      contentEl.dispatchEvent(new MouseEvent('mouseup', { bubbles: true }));
      await new Promise((r) => setTimeout(r, 20));
    });

    await act(async () => {
      container.querySelector('.selection-toolbar button')!.dispatchEvent(
        new MouseEvent('click', { bubbles: true }),
      );
    });

    await waitFor(() => {
      expect(container.querySelector('mark.pending-highlight')).toBeTruthy();
    });

    // Simulate SSE comment event arriving (this is the race condition that used to crash).
    // The server returns a new thread for the same text — React must re-render
    // the MarkdownViewer with both the pending highlight AND the new thread highlight.
    const newThread = {
      id: 'thread-1',
      status: 'open' as const,
      createdAt: '2026-01-01T00:00:00Z',
      anchor: { selectedText: 'test content', startLine: 3, before: '', after: '', headingPath: '' },
      comments: [{ id: 'c1', author: 'user', role: 'human' as const, body: 'comment', createdAt: '' }],
    };
    vi.mocked(api.getThreads).mockResolvedValue([newThread]);

    const { onEvent } = getSSECallbacks();
    expect(onEvent).toBeDefined();

    // Fire SSE event — this triggers fetchThreads → setThreads → re-render.
    // Before the fix, this would crash with "The object can not be found here"
    // because pending highlights were DOM mutations that broke React reconciliation.
    await act(async () => {
      onEvent!({ type: 'comments', project: 'ws/proj' });
      await new Promise((r) => setTimeout(r, 50));
    });

    // Should not have crashed — verify the page still renders
    expect(container.querySelector('#content')).toBeTruthy();
    expect(container.textContent).toContain('Some test content here.');

    // The persisted thread highlight should now also be visible
    await waitFor(() => {
      const marks = container.querySelectorAll('mark.comment-highlight');
      expect(marks.length).toBeGreaterThanOrEqual(1);
    });

    Range.prototype.getBoundingClientRect = origGetBCR;
  });

  it('refreshes content, agent status, and threads on SSE reconnect', async () => {
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);

    renderFilePage();

    // Wait for initial load
    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalledTimes(1);
      expect(api.getThreads).toHaveBeenCalledTimes(1);
      expect(api.getRawFile).toHaveBeenCalledTimes(1);
    });

    // Get the onReconnect callback (2nd argument to useSSE)
    const useSSEMock = vi.mocked(useSSE);
    const onReconnect = useSSEMock.mock.calls[0]?.[1];
    expect(onReconnect).toBeDefined();

    // Simulate SSE reconnect
    vi.mocked(api.getAgentStatus).mockClear();
    vi.mocked(api.getThreads).mockClear();
    vi.mocked(api.getRawFile).mockClear();
    vi.mocked(api.getProjectFiles).mockClear();
    vi.mocked(api.focusFile).mockClear();
    act(() => {
      onReconnect!();
    });

    await waitFor(() => {
      expect(api.focusFile).toHaveBeenCalledWith('ws/proj', 'thoughts/plan.md', undefined);
      expect(api.getRawFile).toHaveBeenCalledTimes(1);
      expect(api.getAgentStatus).toHaveBeenCalledTimes(1);
      expect(api.getThreads).toHaveBeenCalledTimes(1);
      expect(api.getProjectFiles).toHaveBeenCalledTimes(1);
    });
  });

  it('re-fetches file metadata when worktree changes', async () => {
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);
    vi.mocked(api.getProjectFiles).mockResolvedValue([
      { name: 'Thoughts', source: 'thoughts', sourceType: 'thoughts', auto: false, files: [{ path: 'thoughts/plan.md', name: 'plan.md', title: 'Plan', fileType: 'thoughts', age: '1h' }] },
    ]);

    renderFilePageNavigable('/file/ws/proj@wt1/thoughts/plan.md');

    await waitFor(() => {
      expect(api.getProjectFiles).toHaveBeenCalledWith('ws/proj', 'wt1');
    });

    // Navigate to a different worktree on the same file
    vi.mocked(api.getProjectFiles).mockClear();
    await act(async () => {
      testNavigate('/file/ws/proj@wt2/thoughts/plan.md');
    });

    await waitFor(() => {
      expect(api.getProjectFiles).toHaveBeenCalledWith('ws/proj', 'wt2');
    });
  });

  it('refreshes content on SSE files event', async () => {
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);

    renderFilePage();

    // Wait for initial load
    await waitFor(() => {
      expect(api.getRawFile).toHaveBeenCalledTimes(1);
    });

    // Get the onEvent callback (1st argument to useSSE)
    const useSSEMock = vi.mocked(useSSE);
    const onEvent = useSSEMock.mock.calls[0]?.[0];
    expect(onEvent).toBeDefined();

    // Simulate a 'files' SSE event for our project
    vi.mocked(api.getRawFile).mockClear();
    vi.mocked(api.getProjectFiles).mockClear();
    act(() => {
      onEvent!({ type: 'files', project: 'ws/proj' });
    });

    await waitFor(() => {
      expect(api.getRawFile).toHaveBeenCalledTimes(1);
      expect(api.getProjectFiles).toHaveBeenCalledTimes(1);
    });
  });
});
