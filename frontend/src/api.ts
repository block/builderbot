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
} from './types';

export const API_BASE = import.meta.env.VITE_API_URL || '';

export const isDesktopApp = typeof window !== 'undefined' && '__TAURI__' in window;

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

export const api = {
  // Projects
  listProjects: () => apiFetch<APIProject[]>('/api/projects'),
  addProject: (path: string) =>
    apiVoid('/api/projects', { method: 'POST', body: JSON.stringify({ path }) }),
  closeProject: (path: string) =>
    apiVoid('/api/projects', { method: 'DELETE', body: JSON.stringify({ path }) }),

  // Project files
  getProjectFiles: (qn: string) =>
    apiFetch<APIFileGroupView[]>(`/api/project/${qn}`),
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
  getThreads: (project: string, file: string) =>
    apiFetch<ThreadResponse[]>(`/api/threads?project=${encodeURIComponent(project)}&path=${encodeURIComponent(file)}`),
  getAllThreads: (project: string) =>
    apiFetch<ThreadWithFile[]>(`/api/threads?project=${encodeURIComponent(project)}`),
  createThread: (data: CreateThreadReq) =>
    apiFetch<Thread>('/api/threads', { method: 'POST', body: JSON.stringify(data) }),
  replyToThread: (id: string, data: ReplyReq) =>
    apiFetch<Thread>(`/api/threads/${encodeURIComponent(id)}/comments`, {
      method: 'POST',
      body: JSON.stringify(data),
    }),
  patchThread: (id: string, data: PatchThreadReq) =>
    apiFetch<{ ok: boolean }>(`/api/threads/${encodeURIComponent(id)}`, {
      method: 'PATCH',
      body: JSON.stringify(data),
    }),

  // Reviews
  getReviews: (project: string) =>
    apiFetch<APIFileInReview[]>(`/api/reviews?project=${encodeURIComponent(project)}`),

  // Raw file content
  getRawFile: (project: string, path: string) =>
    fetch(`${API_BASE}/api/raw?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}`).then(
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

  // Search
  search: (query: string) =>
    apiFetch<SearchResponse>(`/api/search?q=${encodeURIComponent(query)}`),

  // File operations
  copyFile: (project: string, path: string) =>
    apiVoid(`/api/copy-file?project=${encodeURIComponent(project)}&path=${encodeURIComponent(path)}`),

  // Misc
  open: (path: string) =>
    apiFetch<{ url: string }>('/api/open', { method: 'POST', body: JSON.stringify({ path }) }),
};
