import { useEffect, useState, useCallback } from 'react';
import { Link, NavLink, Outlet, useNavigate } from 'react-router-dom';
import { api, isDesktopApp } from '../api';
import { useTheme } from '../hooks/useTheme';
import { useSSE } from '../hooks/useSSE';
import type { APIProject, SSEEvent } from '../types';

function debounce<T extends (...args: never[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout>;
  return ((...args: Parameters<T>) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  }) as T;
}

export default function Layout() {
  const { theme, toggle } = useTheme();
  const navigate = useNavigate();
  const [projects, setProjects] = useState<APIProject[]>([]);
  const [reviewCount, setReviewCount] = useState(0);
  const [searchQuery, setSearchQuery] = useState('');

  const refreshProjects = useCallback(() => {
    api.listProjects().then(setProjects).catch(() => {});
  }, []);

  const refreshReviewCount = useCallback(() => {
    api.getInReview().then((groups) => {
      const count = groups.reduce((sum, g) => sum + g.files.length, 0);
      setReviewCount(count);
    }).catch(() => {});
  }, []);

  useEffect(() => {
    refreshProjects();
    refreshReviewCount();
  }, [refreshProjects, refreshReviewCount]);

  const debouncedRefreshProjects = useCallback(
    () => debounce(refreshProjects, 200)(),
    [refreshProjects],
  );
  const debouncedRefreshReview = useCallback(
    () => debounce(refreshReviewCount, 200)(),
    [refreshReviewCount],
  );

  useSSE(
    useCallback(
      (event: SSEEvent) => {
        if (event.type === 'projects' || event.type === 'agents') {
          debouncedRefreshProjects();
        }
        if (event.type === 'comments') {
          debouncedRefreshReview();
        }
      },
      [debouncedRefreshProjects, debouncedRefreshReview],
    ),
  );

  // Group projects by workspace
  const workspaceProjects = projects.filter((p) => p.origin === 'workspace');
  const standaloneProjects = projects.filter((p) => p.origin === 'standalone');
  const workspaces = [...new Set(workspaceProjects.map((p) => p.workspace))];

  function handleSearch(e: React.FormEvent) {
    e.preventDefault();
    if (searchQuery.trim()) {
      navigate(`/search?q=${encodeURIComponent(searchQuery.trim())}`);
    }
  }

  return (
    <div className="app" data-testid="app-layout">
      <div
        className="topbar"
        {...(isDesktopApp ? { 'data-tauri-drag-region': '' } : {})}
      >
        <Link to="/" className="topbar-logo">
          Penpal
        </Link>
        <form className="topbar-search" onSubmit={handleSearch}>
          <input
            type="text"
            name="q"
            placeholder="Search files..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
        </form>
        <button className="theme-toggle" onClick={toggle} aria-label="Toggle theme">
          {theme === 'dark' ? '☾' : '☀'}
        </button>
      </div>

      <nav className="sidebar" data-testid="sidebar">
        {workspaces.map((ws) => (
          <NavLink
            key={ws}
            to={`/workspace/${encodeURIComponent(ws)}`}
            className={({ isActive }) => `sidebar-item${isActive ? ' active' : ''}`}
          >
            {ws}
            {workspaceProjects
              .filter((p) => p.workspace === ws)
              .some((p) => p.agentConnected) && <span className="agent-dot" />}
          </NavLink>
        ))}

        {standaloneProjects.map((p) => (
          <NavLink
            key={p.qualifiedName}
            to={`/project/${encodeURIComponent(p.qualifiedName)}`}
            className={({ isActive }) => `sidebar-item${isActive ? ' active' : ''}`}
          >
            {p.name}
            {p.badges.map((b) => (
              <span
                key={b.text}
                className="source-badge"
                style={{ color: b.color, backgroundColor: b.bg }}
              >
                {b.text}
              </span>
            ))}
            {p.agentConnected && <span className="agent-dot" />}
          </NavLink>
        ))}

        <div className="sidebar-divider" />

        <NavLink
          to="/in-review"
          className={({ isActive }) =>
            `sidebar-link${isActive ? ' active' : ''}${reviewCount === 0 ? ' no-reviews' : ''}`
          }
        >
          In Review{reviewCount > 0 && ` (${reviewCount})`}
        </NavLink>
        <NavLink
          to="/recent"
          className={({ isActive }) => `sidebar-link${isActive ? ' active' : ''}`}
        >
          Recent
        </NavLink>
      </nav>

      <div className="main-content">
        <Outlet />
      </div>
    </div>
  );
}
