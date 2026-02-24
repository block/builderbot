import { useEffect, useState, useCallback, useRef, type ReactNode } from 'react';
import { Link, NavLink, Outlet, useNavigate, useLocation } from 'react-router-dom';
import { api, isDesktopApp } from '../api';
import { useTheme } from '../hooks/useTheme';
import { useSSE } from '../hooks/useSSE';
import { useTabs, deriveTitleFromPath } from '../hooks/useTabs';
import { openInNewWindow } from '../utils/window';
import TableOfContents from './TableOfContents';
import FindBar from './FindBar';
import type { Heading } from './TableOfContents';
import type { APIProject, SSEEvent } from '../types';

export interface LayoutContext {
  setHeadings: (headings: Heading[]) => void;
  setSidebarExtra: (node: ReactNode) => void;
  projects: APIProject[];
}

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
  const location = useLocation();
  const { tabs, activeTabId, openTab, closeTab, activateTab } = useTabs();
  const [projects, setProjects] = useState<APIProject[]>([]);
  const [reviewCount, setReviewCount] = useState(0);
  const [searchQuery, setSearchQuery] = useState('');
  const [headings, setHeadings] = useState<Heading[]>([]);
  const [sidebarExtra, setSidebarExtra] = useState<ReactNode>(null);
  const isFilePage = location.pathname.startsWith('/file/');

  // Add modal state
  const [showAddModal, setShowAddModal] = useState(false);
  const [addPath, setAddPath] = useState('');
  const [addError, setAddError] = useState('');
  const [addLoading, setAddLoading] = useState(false);

  // Find bar state
  const [showFindBar, setShowFindBar] = useState(false);

  // Sidebar three-dot menu state
  const [openSidebarMenu, setOpenSidebarMenu] = useState<string | null>(null);
  const sidebarMenuRef = useRef<HTMLDivElement>(null);

  // Clear headings when navigating away from file pages
  useEffect(() => {
    if (!isFilePage) setHeadings([]);
  }, [isFilePage]);

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

  // Close sidebar menu on outside click
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (sidebarMenuRef.current && !sidebarMenuRef.current.contains(e.target as Node)) {
        setOpenSidebarMenu(null);
      }
    }
    if (openSidebarMenu) {
      document.addEventListener('mousedown', handleClick);
      return () => document.removeEventListener('mousedown', handleClick);
    }
  }, [openSidebarMenu]);

  // Group projects by workspace
  const workspaceProjects = projects.filter((p) => p.origin === 'workspace');
  const standaloneProjects = projects.filter((p) => p.origin === 'standalone');
  const workspaces = [...new Set(workspaceProjects.map((p) => p.workspace))];

  // Listen for native menu events (tab/window shortcuts)
  useEffect(() => {
    async function handleCloseTab() {
      if (tabs.length <= 1) {
        // Last tab — close the window
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        getCurrentWindow().close();
      } else {
        closeTab(activeTabId);
      }
    }
    function handleNewTab() {
      const ws = workspaces[0];
      if (ws) {
        openTab(`/workspace/${encodeURIComponent(ws)}`, ws);
      } else if (standaloneProjects[0]) {
        openTab(`/project/${standaloneProjects[0].qualifiedName}`, standaloneProjects[0].name);
      } else {
        openTab('/');
      }
    }
    function handlePrevTab() {
      const idx = tabs.findIndex((t) => t.id === activeTabId);
      if (idx > 0) activateTab(tabs[idx - 1].id);
      else if (tabs.length > 0) activateTab(tabs[tabs.length - 1].id);
    }
    function handleNextTab() {
      const idx = tabs.findIndex((t) => t.id === activeTabId);
      if (idx < tabs.length - 1) activateTab(tabs[idx + 1].id);
      else if (tabs.length > 0) activateTab(tabs[0].id);
    }
    window.addEventListener('menu-close-tab', handleCloseTab);
    window.addEventListener('menu-new-tab', handleNewTab);
    window.addEventListener('menu-prev-tab', handlePrevTab);
    window.addEventListener('menu-next-tab', handleNextTab);
    return () => {
      window.removeEventListener('menu-close-tab', handleCloseTab);
      window.removeEventListener('menu-new-tab', handleNewTab);
      window.removeEventListener('menu-prev-tab', handlePrevTab);
      window.removeEventListener('menu-next-tab', handleNextTab);
    };
  }, [activeTabId, closeTab, openTab, activateTab, tabs, workspaces, standaloneProjects]);

  // Listen for find bar toggle (Tauri menu event only — in browser, native Cmd+F works)
  useEffect(() => {
    if (!isDesktopApp) return;
    function handleMenuFind() {
      setShowFindBar((prev) => !prev);
    }
    window.addEventListener('menu-find', handleMenuFind);
    return () => {
      window.removeEventListener('menu-find', handleMenuFind);
    };
  }, []);

  // Detect project-mode view: /project/:qn or /file/:qn/*
  // QN may contain slashes (e.g. "Development/birdseye"), so match against known projects
  const pathAfterPrefix = location.pathname.match(/^\/(project|file)\/(.+)/)?.[2] || '';
  const activeProject = pathAfterPrefix
    ? projects.find((p) => pathAfterPrefix === p.qualifiedName || pathAfterPrefix.startsWith(p.qualifiedName + '/'))
    : null;
  // Show project-mode sidebar as soon as URL matches, even before projects load
  const isProjectMode = !!pathAfterPrefix;

  function handleSearch(e: React.FormEvent) {
    e.preventDefault();
    if (searchQuery.trim()) {
      navigate(`/search?q=${encodeURIComponent(searchQuery.trim())}`);
    }
  }

  function handleAddWorkspace() {
    if (!addPath.trim()) return;
    setAddLoading(true);
    setAddError('');
    api.addWorkspace(addPath.trim())
      .then(() => {
        refreshProjects();
        const name = addPath.trim().split('/').pop() || addPath.trim();
        setShowAddModal(false);
        setAddPath('');
        navigate(`/workspace/${encodeURIComponent(name)}`);
      })
      .catch((err) => setAddError(err.message))
      .finally(() => setAddLoading(false));
  }

  function handleAddProject() {
    if (!addPath.trim()) return;
    setAddLoading(true);
    setAddError('');
    api.addProject(addPath.trim())
      .then(() => {
        refreshProjects();
        setShowAddModal(false);
        setAddPath('');
        // Navigate after refresh settles
        api.listProjects().then((all) => {
          const added = all.find((p) => p.projectPath === addPath.trim());
          if (added) navigate(`/project/${added.qualifiedName}`);
        });
      })
      .catch((err) => setAddError(err.message))
      .finally(() => setAddLoading(false));
  }

  function handleRemoveWorkspace(ws: string) {
    setOpenSidebarMenu(null);
    const wsProject = workspaceProjects.find((p) => p.workspace === ws);
    const wsPath = wsProject?.workspacePath;
    if (!wsPath) return;
    api.removeWorkspace(wsPath)
      .then(() => {
        refreshProjects();
        if (location.pathname === `/workspace/${encodeURIComponent(ws)}`) {
          navigate('/');
        }
      })
      .catch((err) => alert('Failed to remove workspace: ' + err.message));
  }

  function handleCloseStandaloneProject(p: APIProject) {
    setOpenSidebarMenu(null);
    api.closeProject(p.projectPath)
      .then(() => {
        refreshProjects();
        if (location.pathname.startsWith(`/project/${p.qualifiedName}`)) {
          navigate('/');
        }
      })
      .catch((err) => alert('Failed to close project: ' + err.message));
  }

  // Intercept clicks on links:
  // - External links (http/https) → open in default browser (desktop only)
  // - Cmd/Ctrl+click on internal links → new tab or new window
  function handleAppClick(e: React.MouseEvent) {
    const target = (e.target as HTMLElement).closest('a');
    if (!target) return;
    const href = target.getAttribute('href');
    if (!href) return;

    // External links: open in default browser on desktop
    if (href.startsWith('http://') || href.startsWith('https://') || href.startsWith('//')) {
      if (isDesktopApp) {
        e.preventDefault();
        e.stopPropagation();
        import('@tauri-apps/plugin-shell').then(({ open }) => open(href));
      }
      return;
    }

    // Internal links: only intercept cmd/ctrl+click
    if (!e.metaKey && !e.ctrlKey) return;
    e.preventDefault();
    e.stopPropagation();
    const title = deriveTitleFromPath(href);

    if (e.shiftKey) {
      openInNewWindow(href, title);
    } else {
      openTab(href, title, { background: true });
    }
  }

  function renderSidebarMenu(id: string, items: { label: string; className?: string; onClick: () => void }[]) {
    return (
      <div className="sidebar-menu-wrap" ref={openSidebarMenu === id ? sidebarMenuRef : undefined}>
        <button
          className="sidebar-dots"
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            setOpenSidebarMenu(openSidebarMenu === id ? null : id);
          }}
        >
          &#8942;
        </button>
        {openSidebarMenu === id && (
          <div className="dropdown-menu">
            {items.map((item) => (
              <button
                key={item.label}
                className={item.className}
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  item.onClick();
                }}
              >
                {item.label}
              </button>
            ))}
          </div>
        )}
      </div>
    );
  }

  const outletContext: LayoutContext = { setHeadings, setSidebarExtra, projects };

  return (
    <div className="app" data-testid="app-layout" onClick={handleAppClick}>
      <div
        className="topbar"
        {...(isDesktopApp ? { 'data-tauri-drag-region': '' } : {})}
      >
        <Link to="/" className="topbar-logo">
          Penpal
        </Link>
        <form className="topbar-search" onSubmit={handleSearch}>
          <input
            type="search"
            name="q"
            placeholder="Search all thoughts..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
        </form>
        <button className="theme-toggle" onClick={toggle} aria-label="Toggle dark mode" title="Toggle dark mode">
          {theme === 'dark' ? '☾' : '☀'}
        </button>
      </div>

      <div className="tab-bar" data-testid="topbar-tabs">
        {tabs.map(tab => (
          <button
            key={tab.id}
            className={`tab-bar-tab${tab.id === activeTabId ? ' active' : ''}`}
            onClick={() => activateTab(tab.id)}
            onAuxClick={(e) => { if (e.button === 1) closeTab(tab.id); }}
          >
            <span className="tab-title">{tab.title}</span>
            {tabs.length > 1 && (
              <span className="tab-close" onClick={(e) => { e.stopPropagation(); closeTab(tab.id); }}>×</span>
            )}
          </button>
        ))}
        <button className="tab-bar-new" onClick={() => {
          const ws = workspaces[0];
          if (ws) {
            openTab(`/workspace/${encodeURIComponent(ws)}`, ws);
          } else if (standaloneProjects[0]) {
            openTab(`/project/${standaloneProjects[0].qualifiedName}`, standaloneProjects[0].name);
          } else {
            openTab('/');
          }
        }} aria-label="New tab">+</button>
      </div>

      <nav className="sidebar" data-testid="sidebar">
        {isProjectMode ? (
          <>
            <Link to={activeProject?.origin === 'workspace' && activeProject.workspace
              ? `/workspace/${encodeURIComponent(activeProject.workspace)}`
              : '/'
            } className="sidebar-item sidebar-back">← Home</Link>
            <div className="sidebar-divider" />
            {activeProject && activeProject.origin === 'workspace' && activeProject.workspace && (
              <NavLink
                to={`/workspace/${encodeURIComponent(activeProject.workspace)}`}
                className="sidebar-item"
              >
                {activeProject.workspace}
                {workspaceProjects
                  .filter((p) => p.workspace === activeProject.workspace)
                  .some((p) => p.agentConnected) && <span className="agent-dot" />}
              </NavLink>
            )}
            {activeProject && (
              <NavLink
                to={`/project/${activeProject.qualifiedName}`}
                className="sidebar-item subitem active"
              >
                {activeProject.name}
                {activeProject.badges.map((b) => (
                  <span
                    key={b.text}
                    className="source-badge"
                    style={{ '--badge-bg': b.bg, '--badge-color': b.color, '--badge-active-bg': b.activeBg || b.bg, '--badge-active-color': b.activeColor || b.color } as React.CSSProperties}
                  >
                    {b.text}
                  </span>
                ))}
                {activeProject.agentConnected && <span className="agent-dot" />}
                {activeProject.branch && (
                  <span className="branch-info">
                    {activeProject.branch}
                    {activeProject.dirty && <span className="branch-dirty">*</span>}
                  </span>
                )}
              </NavLink>
            )}
          </>
        ) : (
          <>
            {workspaces.map((ws) => (
              <div key={ws} className={`sidebar-item-row${openSidebarMenu === `ws-${ws}` ? ' menu-open' : ''}`}>
                <NavLink
                  to={`/workspace/${encodeURIComponent(ws)}`}
                  className={({ isActive }) => `sidebar-item${isActive ? ' active' : ''}`}
                >
                  {ws}
                  {workspaceProjects
                    .filter((p) => p.workspace === ws)
                    .some((p) => p.agentConnected) && <span className="agent-dot" />}
                </NavLink>
                {renderSidebarMenu(`ws-${ws}`, [
                  { label: 'Remove workspace', className: 'menu-danger', onClick: () => handleRemoveWorkspace(ws) },
                ])}
              </div>
            ))}

            {standaloneProjects.map((p) => (
              <div key={p.qualifiedName} className={`sidebar-item-row${openSidebarMenu === `proj-${p.qualifiedName}` ? ' menu-open' : ''}`}>
                <NavLink
                  to={`/project/${p.qualifiedName}`}
                  className={({ isActive }) => `sidebar-item${isActive ? ' active' : ''}`}
                >
                  {p.name}
                  {p.badges.map((b) => (
                    <span
                      key={b.text}
                      className="source-badge"
                      style={{ '--badge-bg': b.bg, '--badge-color': b.color, '--badge-active-bg': b.activeBg || b.bg, '--badge-active-color': b.activeColor || b.color } as React.CSSProperties}
                    >
                      {b.text}
                    </span>
                  ))}
                  {p.agentConnected && <span className="agent-dot" />}
                </NavLink>
                {renderSidebarMenu(`proj-${p.qualifiedName}`, [
                  { label: 'Close project', className: 'menu-muted', onClick: () => handleCloseStandaloneProject(p) },
                ])}
              </div>
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
            <button
              className="sidebar-add-btn"
              onClick={() => { setShowAddModal(true); setAddPath(''); setAddError(''); }}
            >
              + Add workspace or project
            </button>
          </>
        )}

        {sidebarExtra}
        {isFilePage && headings.length > 0 && <TableOfContents headings={headings} />}
      </nav>

      <div className="main-content" style={isFilePage ? { padding: 0, overflow: 'hidden' } : undefined}>
        {isDesktopApp && showFindBar && <FindBar onClose={() => setShowFindBar(false)} />}
        <Outlet context={outletContext} />
      </div>

      {/* Add workspace/project modal */}
      <div className={`modal-overlay${showAddModal ? ' open' : ''}`} onClick={() => setShowAddModal(false)}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <h3>Add workspace or project</h3>
          <p>Enter the absolute path to a directory.</p>
          <input
            className="modal-input"
            type="text"
            placeholder="/home/user/projects"
            value={addPath}
            onChange={(e) => setAddPath(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter') handleAddWorkspace(); }}
            autoFocus
          />
          <div className={`modal-error${addError ? ' visible' : ''}`}>{addError}</div>
          <div className="modal-actions">
            <button className="btn-cancel" onClick={() => setShowAddModal(false)}>Cancel</button>
            <button className="btn-primary" onClick={handleAddProject} disabled={!addPath.trim() || addLoading}>
              Add Project
            </button>
            <button className="btn-primary" onClick={handleAddWorkspace} disabled={!addPath.trim() || addLoading}>
              Add Workspace
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
