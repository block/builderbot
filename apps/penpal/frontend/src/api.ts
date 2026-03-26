import type {
  APIProject,
  APIFileGroupView,
  APIFile,
  ReviewGroup,
  ThreadResponse,
  ThreadWithFile,
  Thread,
  CreateThreadReq,
  ReplyReq,
  PatchThreadReq,
  APIFileInReview,
  ProjectInfo,
  AgentStatus,
  PublishState,
  SearchResponse,
  InstallToolsStatus,
} from './types';

export const API_BASE = import.meta.env.VITE_API_URL || '';

export const isDesktopApp = typeof window !== 'undefined' && '__TAURI__' in window;

const WINDOW_FOCUS_KEY = 'penpal-window-focus-id';
let inMemoryWindowFocusID: string | null = null;
let resolvedWindowFocusID: string | null = null;
let windowFocusIDPromise: Promise<string> | null = null;

async function apiFetch<T>(path: string, options?: RequestInit): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    ...options,
    headers: { 'Content-Type': 'application/json', ...options?.headers },
  });
  if (!res.ok) throw new Error(`API error: ${res.status}`);
  return res.json();
}

async function apiVoid(path: string, options?: RequestInit): Promise<void> {
  const res = await fetch(`${API_BASE}${path}`, {
    ...options,
    headers: { 'Content-Type': 'application/json', ...options?.headers },
  });
  if (!res.ok) throw new Error(`API error: ${res.status}`);
}

function wtParam(worktree?: string): string {
  return worktree ? `&worktree=${encodeURIComponent(worktree)}` : '';
}

function generateWindowFocusID(): string {
  return typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `win-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function getBrowserWindowFocusID(): string {
  if (resolvedWindowFocusID) return resolvedWindowFocusID;
  try {
    const existing = window.sessionStorage.getItem(WINDOW_FOCUS_KEY);
    if (existing) {
      resolvedWindowFocusID = existing;
      return existing;
    }
    const generated = generateWindowFocusID();
    window.sessionStorage.setItem(WINDOW_FOCUS_KEY, generated);
    resolvedWindowFocusID = generated;
    return generated;
  } catch {
    if (!inMemoryWindowFocusID) inMemoryWindowFocusID = generateWindowFocusID();
    resolvedWindowFocusID = inMemoryWindowFocusID;
    return inMemoryWindowFocusID;
  }
}

async function getWindowFocusID(): Promise<string> {
  if (resolvedWindowFocusID) return resolvedWindowFocusID;
  if (typeof window === 'undefined') {
    resolvedWindowFocusID = generateWindowFocusID();
    return resolvedWindowFocusID;
  }
  if (!isDesktopApp) return getBrowserWindowFocusID();
  if (!windowFocusIDPromise) {
    windowFocusIDPromise = import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => {
        const label = getCurrentWindow().label || generateWindowFocusID();
        resolvedWindowFocusID = label;
        return label;
      })
      .catch(() => getBrowserWindowFocusID());
  }
  return windowFocusIDPromise;
}

async function focusWindowParam(): Promise<string> {
  return `window=${encodeURIComponent(await getWindowFocusID())}`;
}

export const api = {
  // Projects
  listProjects: () => apiFetch<APIProject[]>('/api/projects'),
  addProject: (path: string) =>
    apiVoid('/api/projects', { method: 'POST', body: JSON.stringify({ path }) }),
  closeProject: (path: string) =>
    apiVoid('/api/projects', { method: 'DELETE', body: JSON.stringify({ path }) }),

  // Project files
  getProjectFiles: (qn: string, worktree?: string) =>
    apiFetch<APIFileGroupView[]>(`/api/project/${qn}${worktree ? '?worktree=' + encodeURIComponent(worktree) : ''}`),
  getProjectInfo: (name: string) =>
    apiFetch<ProjectInfo>(`/api/project-info?name=${encodeURIComponent(name)}`),
  deleteProject: (project: string) =>
    apiVoid(`/api/delete-project?name=${encodeURIComponent(project)}`, { method: 'POST' }),
  deleteFile: (project: string, path: string) =>
    apiVoid(`/api/delete-file?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}`, { method: 'POST' }),

  // Recent / In-review
  getRecentFiles: () => apiFetch<APIFile[]>('/api/recent'),
  getInReview: () => apiFetch<ReviewGroup[]>('/api/in-review'),

  // Threads
  getThreads: (project: string, file: string, worktree?: string) =>
    apiFetch<ThreadResponse[]>(`/api/threads?project=${encodeURIComponent(project)}&path=${encodeURIComponent(file)}${wtParam(worktree)}`),
  getAllThreads: (project: string) =>
    apiFetch<ThreadWithFile[]>(`/api/threads?project=${encodeURIComponent(project)}`),
  createThread: (data: CreateThreadReq & { worktree?: string }) =>
    apiFetch<Thread>('/api/threads', { method: 'POST', body: JSON.stringify(data) }),
  replyToThread: (id: string, data: ReplyReq & { worktree?: string }) =>
    apiFetch<Thread>(`/api/threads/${encodeURIComponent(id)}/comments`, {
      method: 'POST',
      body: JSON.stringify(data),
    }),
  patchThread: (id: string, data: PatchThreadReq & { worktree?: string }) =>
    apiFetch<{ ok: boolean }>(`/api/threads/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    }),

  // Reviews
  getReviews: (project: string, worktree?: string) =>
    apiFetch<APIFileInReview[]>(`/api/reviews?project=${encodeURIComponent(project)}${wtParam(worktree)}`),

  // Raw file content
  getRawFile: (project: string, path: string, worktree?: string) =>
    fetch(`${API_BASE}/api/raw?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}${wtParam(worktree)}`, {
      cache: 'no-store',
    }).then(
      (r) => {
        if (!r.ok) throw new Error(`API error: ${r.status}`);
        return r.text();
      },
    ),

  // Agents
  getAgentStatus: (project: string) =>
    apiFetch<AgentStatus>(`/api/agents?project=${encodeURIComponent(project)}`),
  startAgent: (project: string) =>
    apiFetch<AgentStatus>(`/api/agents/start?project=${encodeURIComponent(project)}`, {
      method: 'POST',
    }),
  stopAgent: (project: string) =>
    apiVoid(`/api/agents/stop?project=${encodeURIComponent(project)}`, { method: 'POST' }),

  // Workspaces
  addWorkspace: (path: string) =>
    apiVoid('/api/workspaces', { method: 'POST', body: JSON.stringify({ path }) }),
  removeWorkspace: (path: string) =>
    apiVoid('/api/workspaces', { method: 'DELETE', body: JSON.stringify({ path }) }),

  // Sources
  addSource: (project: string, path: string, name?: string) =>
    apiVoid('/api/sources', { method: 'POST', body: JSON.stringify({ project, path, name }) }),
  removeSource: (project: string, name?: string, file?: string) =>
    apiVoid('/api/sources', { method: 'DELETE', body: JSON.stringify({ project, name, file }) }),

  // Publish
  publish: (project: string, path: string) =>
    apiFetch<{ url: string; siteName: string }>('/api/publish', {
      method: 'POST',
      body: JSON.stringify({ project, path }),
    }),
  getPublishState: (project: string, path: string) =>
    apiFetch<PublishState>(
      `/api/publish-state?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}`,
    ),

  // Activity tracking
  recordView: (project: string, path: string) =>
    apiVoid(`/api/view?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}`, { method: 'POST' }),

  // Search
  search: (query: string) =>
    apiFetch<SearchResponse>(`/api/search?q=${encodeURIComponent(query)}`),

  // File operations
  copyFile: (project: string, path: string) =>
    apiVoid(`/api/copy-file?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}`),

  // Install tools
  checkInstallStatus: () => apiFetch<InstallToolsStatus>('/api/install-tools'),
  installTools: () =>
    apiFetch<InstallToolsStatus>('/api/install-tools', { method: 'POST' }),
  setClaudePath: (path: string) =>
    apiFetch<{ path: string; version: string }>('/api/claude-path', {
      method: 'PUT',
      body: JSON.stringify({ path }),
    }),

  // Focus (tells server what to deep-watch for the current view)
  focusProject: async (project: string) =>
    apiVoid(`/api/focus?${await focusWindowParam()}&project=${encodeURIComponent(project)}`, { method: 'POST' }),
  focusFile: async (project: string, path: string, worktree?: string) =>
    apiVoid(`/api/focus?${await focusWindowParam()}&project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}${wtParam(worktree)}`, { method: 'POST' }),
  clearFocus: async (options?: RequestInit) =>
    apiVoid(`/api/focus?${await focusWindowParam()}`, { method: 'DELETE', ...options }),

  // Misc
  open: (path: string) =>
    apiFetch<{ url: string }>('/api/open', { method: 'POST', body: JSON.stringify({ path }) }),
};
