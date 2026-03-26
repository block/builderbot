import { useEffect, useMemo, useState, useCallback, useRef, type ReactNode } from 'react';
import { Link, NavLink, Outlet, useNavigate, useLocation } from 'react-router-dom';
import { api, isDesktopApp, API_BASE } from '../api';
import { useTheme } from '../hooks/useTheme';
import { useSSE } from '../hooks/useSSE';
import { useTabs, deriveTitleFromPath } from '../hooks/useTabs';
import { openInNewWindow } from '../utils/window';
import TableOfContents from './TableOfContents';
import FindBar from './FindBar';
import InstallToolsModal from './InstallToolsModal';
import type { Heading } from './TableOfContents';
import type { APIProject, SSEEvent } from '../types';
import { parseProjectWorktree } from '../utils/worktree';
import { useProjectSort } from '../hooks/useProjectSort';

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
  const { tabs, activeTabId, openTab, closeTab, activateTab, canGoBack, canGoForward, goBack, goForward } = useTabs();
  const tabsRef = useRef(tabs);
  useEffect(() => { tabsRef.current = tabs; }, [tabs]);
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

  // Install tools modal state
  const [showInstallModal, setShowInstallModal] = useState(false);

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
      const count = groups.reduce((sum, g) => sum + (g.files?.length ?? 0), 0);
      setReviewCount(count);
    }).catch(() => {});
  }, []);

  const clearWindowFocusOnClose = useCallback(
    (options?: RequestInit) => api.clearFocus(options).catch(() => {}),
    [],
  );

  useEffect(() => {
    refreshProjects();
    refreshReviewCount();
  }, [refreshProjects, refreshReviewCount]);

  useEffect(() => {
    const handlePageHide = () => {
      clearWindowFocusOnClose({ keepalive: true });
    };

    window.addEventListener('pagehide', handlePageHide);

    let cancelled = false;
    let unlistenCloseRequested: (() => void) | undefined;

    if (isDesktopApp) {
      import('@tauri-apps/api/window')
        .then(({ getCurrentWindow }) => getCurrentWindow().onCloseRequested(async () => {
          await clearWindowFocusOnClose();
        }))
        .then((unlisten) => {
          if (cancelled) {
            unlisten();
            return;
          }
          unlistenCloseRequested = unlisten;
        })
        .catch(() => {});
    }

    return () => {
      cancelled = true;
      window.removeEventListener('pagehide', handlePageHide);
      unlistenCloseRequested?.();
    };
  }, [clearWindowFocusOnClose]);

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
        if (event.type === 'navigate' && event.path) {
          const existing = tabs.find(t => t.path === event.path);
          if (existing) {
            activateTab(existing.id);
          } else {
            openTab(event.path);
          }
        }
      },
      [debouncedRefreshProjects, debouncedRefreshReview, tabs, openTab, activateTab],
    ),
    useCallback(async () => {
      refreshProjects();
      refreshReviewCount();
      // Check for pending navigation that fired while SSE was disconnected
      try {
        const res = await fetch(`${API_BASE}/api/navigate`);
        if (res.ok) {
          const data = await res.json();
          if (data.url) {
            const existing = tabsRef.current.find(t => t.path === data.url);
            if (existing) {
              activateTab(existing.id);
            } else {
              openTab(data.url);
            }
          }
        }
      } catch { /* ignore */ }
    }, [refreshProjects, refreshReviewCount, openTab, activateTab]),
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

  const { sortOrder } = useProjectSort();

  // Group projects by workspace
  const workspaceProjects = useMemo(
    () => projects.filter((p) => p.origin === 'workspace'),
    [projects],
  );
  const standaloneProjects = useMemo(() => {
    const sp = projects.filter((p) => p.origin === 'standalone');
    if (sortOrder === 'alpha') sp.sort((a, b) => a.name.localeCompare(b.name));
    return sp;
  }, [projects, sortOrder]);
  const workspaces = useMemo(() => {
    const ws = [...new Set(workspaceProjects.map((p) => p.workspace))];
    if (sortOrder === 'alpha') ws.sort((a, b) => a.localeCompare(b));
    return ws;
  }, [workspaceProjects, sortOrder]);

  // Listen for native menu events (tab/window shortcuts)
  useEffect(() => {
    async function handleCloseTab() {
      if (tabs.length <= 1) {
        // Last tab — close the window
        await clearWindowFocusOnClose();
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
    window.addEventListener('menu-go-back', goBack);
    window.addEventListener('menu-go-forward', goForward);
    return () => {
      window.removeEventListener('menu-close-tab', handleCloseTab);
      window.removeEventListener('menu-new-tab', handleNewTab);
      window.removeEventListener('menu-prev-tab', handlePrevTab);
      window.removeEventListener('menu-next-tab', handleNextTab);
      window.removeEventListener('menu-go-back', goBack);
      window.removeEventListener('menu-go-forward', goForward);
    };
  }, [activeTabId, clearWindowFocusOnClose, closeTab, openTab, activateTab, tabs, workspaces, standaloneProjects, goBack, goForward]);

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

  // Listen for install tools menu event
  useEffect(() => {
    if (!isDesktopApp) return;
    function handleMenuInstallTools() {
      setShowInstallModal(true);
    }
    window.addEventListener('menu-install-tools', handleMenuInstallTools);
    return () => {
      window.removeEventListener('menu-install-tools', handleMenuInstallTools);
    };
  }, []);

  // On startup, check install status to decide whether to show the modal.
  // The dismiss key is only written after a successful install for this build,
  // so the modal keeps prompting until tools are actually installed and current.
  const [toolsInstalled, setToolsInstalled] = useState(false);
  useEffect(() => {
    if (!isDesktopApp) return;
    const dismissKey = `penpal-install-dismissed-${__BUILD_ID__}`;
    api.checkInstallStatus()
      .then((status) => {
        const hasTools = status.cli.installed || status.plugin.installed;
        setToolsInstalled(hasTools);
        if (!localStorage.getItem(dismissKey)) {
          setShowInstallModal(true);
        }
      })
      .catch(() => {});
  }, []);

  function handleInstallModalClose(installed: boolean) {
    setShowInstallModal(false);
    if (installed) {
      // Only persist dismiss when tools were confirmed installed for this build.
      // This means the modal keeps prompting on startup until tools are actually
      // installed and up-to-date — no stale opt-out dismiss path.
      localStorage.setItem(`penpal-install-dismissed-${__BUILD_ID__}`, '1');
      api.checkInstallStatus()
        .then((status) => {
          setToolsInstalled(status.cli.installed || status.plugin.installed);
        })
        .catch(() => {});
    }
  }


  // Keyboard shortcuts for back/forward in browser mode
  useEffect(() => {
    if (isDesktopApp) return;
    function handleKeyDown(e: KeyboardEvent) {
      if (!e.metaKey && !e.ctrlKey) return;
      if (e.key === '[' && canGoBack) {
        e.preventDefault();
        goBack();
      } else if (e.key === ']' && canGoForward) {
        e.preventDefault();
        goForward();
      }
    }
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [goBack, goForward, canGoBack, canGoForward]);

  // Detect project-mode view: /project/:qn[@worktree] or /file/:qn[@worktree]/*
  // QN may contain slashes (e.g. "Development/birdseye"), so match against known projects
  const pathAfterPrefix = location.pathname.match(/^\/(project|file)\/(.+)/)?.[2] || '';
  const { activeProject, activeWorktree } = (() => {
    if (!pathAfterPrefix) return { activeProject: null, activeWorktree: '' };
    // Strip @worktree suffix before matching against project QNs
    const sorted = [...projects].sort((a, b) => b.qualifiedName.length - a.qualifiedName.length);
    for (const p of sorted) {
      // Check for exact match or prefix match (with / or @ following)
      if (
        pathAfterPrefix === p.qualifiedName ||
        pathAfterPrefix.startsWith(p.qualifiedName + '/') ||
        pathAfterPrefix.startsWith(p.qualifiedName + '@')
      ) {
        const rest = pathAfterPrefix.slice(p.qualifiedName.length);
        const { worktree } = parseProjectWorktree(p.qualifiedName + rest.split('/')[0]);
        return { activeProject: p, activeWorktree: worktree };
      }
    }
    // Fallback: try parsing with @
    const parsed = parseProjectWorktree(pathAfterPrefix.split('/').slice(0, 2).join('/'));
    const fallbackProject = projects.find((p) => p.qualifiedName === parsed.project) || null;
    return { activeProject: fallbackProject, activeWorktree: parsed.worktree };
  })();
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
  // - Regular click on internal links → client-side navigation (preserves tab history)
  function handleAppClick(e: React.MouseEvent) {
    if (e.defaultPrevented) return; // Already handled (e.g. React Router <Link>)
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

    // Hash-only links: use default browser behavior (scroll to anchor)
    // Non-HTTP schemes (mailto:, tel:, etc.): let the browser/OS handle them
    if (href.startsWith('#') || /^[a-z][a-z0-9+.-]*:/i.test(href)) return;

    // Resolve relative hrefs (e.g. ./other-file.md) to absolute paths
    const resolved = new URL(target.href);
    const fullPath = resolved.pathname + resolved.search + resolved.hash;
    // Strip the router basename so navigate/openTab receive router-relative paths
    // (the browser pathname includes the deploy prefix, but the router already prepends it)
    const base = import.meta.env.BASE_URL.replace(/\/+$/, '');
    const resolvedPath = base && fullPath.startsWith(base)
      ? fullPath.slice(base.length) || '/'
      : fullPath;

    // Cmd/Ctrl+click → new tab or new window
    if (e.metaKey || e.ctrlKey) {
      e.preventDefault();
      e.stopPropagation();
      const title = deriveTitleFromPath(resolvedPath);
      if (e.shiftKey) {
        openInNewWindow(resolvedPath, title);
      } else {
        openTab(resolvedPath, title, { background: true });
      }
      return;
    }

    // Regular click on internal link: use client-side navigation to preserve
    // SPA state (tab history, etc.) instead of a full page reload.
    e.preventDefault();
    navigate(resolvedPath);
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
        <button className="topbar-nav" disabled={!canGoBack} onClick={goBack} aria-label="Go back">‹</button>
        <button className="topbar-nav" disabled={!canGoForward} onClick={goForward} aria-label="Go forward">›</button>
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
            {activeProject && activeProject.worktrees && activeProject.worktrees.length > 1 ? (
              <>
                {activeProject.worktrees.map((wt) => {
                  const isActive = wt.isMain ? !activeWorktree : activeWorktree === wt.name;
                  const url = wt.isMain
                    ? `/project/${activeProject.qualifiedName}`
                    : `/project/${activeProject.qualifiedName}@${wt.name}`;
                  return (
                    <NavLink
                      key={wt.name}
                      to={url}
                      className={`sidebar-item subitem worktree-item${isActive ? ' active' : ''}`}
                    >
                      <span className="worktree-name">
                        {wt.isMain ? activeProject.name : wt.name}
                        {wt.isMain && activeProject.badges.map((b) => (
                          <span
                            key={b.text}
                            className="source-badge"
                            style={{ '--badge-bg': b.bg, '--badge-color': b.color, '--badge-active-bg': b.activeBg || b.bg, '--badge-active-color': b.activeColor || b.color } as React.CSSProperties}
                          >
                            {b.text}
                          </span>
                        ))}
                      </span>
                      {wt.branch && (
                        <span className="branch-name">{wt.branch}</span>
                      )}
                    </NavLink>
                  );
                })}
              </>
            ) : activeProject ? (
              <NavLink
                to={`/project/${activeProject.qualifiedName}`}
                className="sidebar-item subitem worktree-item active"
              >
                <span className="worktree-name">
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
                </span>
                {activeProject.branch && (
                  <span className="branch-name">
                    {activeProject.branch}
                    {activeProject.dirty && <span className="branch-dirty">*</span>}
                  </span>
                )}
              </NavLink>
            ) : null}
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

      {isDesktopApp && (
        <InstallToolsModal open={showInstallModal} isUpdate={toolsInstalled} onClose={handleInstallModalClose} />
      )}

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
