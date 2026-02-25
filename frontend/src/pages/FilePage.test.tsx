import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, waitFor, act } from '@testing-library/react';
import { MemoryRouter, Routes, Route, Outlet } from 'react-router-dom';
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
    setSidebarExtra: vi.fn(),
    projects: [{ name: 'proj', qualifiedName: 'ws/proj', workspace: 'ws', origin: 'workspace' as const, badges: [], fileCount: 1, lastModified: '', projectPath: '/tmp/proj' }],
  };
  return <Outlet context={ctx} />;
}

function renderFilePage() {
  return render(
    <MemoryRouter initialEntries={['/file/ws/proj/thoughts/plan.md']}>
      <Routes>
        <Route element={<LayoutWrapper />}>
          <Route path="/file/*" element={<FilePage />} />
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

  it('refreshes agent status and threads on SSE reconnect', async () => {
    vi.mocked(api.getAgentStatus).mockResolvedValue(agentNotRunning);

    renderFilePage();

    // Wait for initial load
    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalledTimes(1);
      expect(api.getThreads).toHaveBeenCalledTimes(1);
    });

    // Get the onReconnect callback (2nd argument to useSSE)
    const useSSEMock = vi.mocked(useSSE);
    const onReconnect = useSSEMock.mock.calls[0]?.[1];
    expect(onReconnect).toBeDefined();

    // Simulate SSE reconnect
    vi.mocked(api.getAgentStatus).mockClear();
    vi.mocked(api.getThreads).mockClear();
    act(() => {
      onReconnect!();
    });

    await waitFor(() => {
      expect(api.getAgentStatus).toHaveBeenCalledTimes(1);
      expect(api.getThreads).toHaveBeenCalledTimes(1);
    });
  });
});
