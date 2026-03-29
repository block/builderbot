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
import ContextMenu, { type ContextMenuItem } from './ContextMenu';
import type { Heading } from './TableOfContents';
import type { APIProject, APIFileGroupView, APIFileInReview, SSEEvent } from '../types';
import { parseProjectWorktree } from '../utils/worktree';
import { useProjectSort } from '../hooks/useProjectSort';

export interface LayoutContext {
  setHeadings: (headings: Heading[]) => void;
  projects: APIProject[];
}

function debounce<T extends (...args: never[]) => void>(fn: T, ms: number): T {
  let timer: ReturnType<typeof setTimeout>;
  return ((...args: Parameters<T>) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn(...args), ms);
  }) as T;
}

// E-PENPAL-HOME-SIDEBAR: home view sidebar tree with workspaces, standalone projects, global nav links.
// E-PENPAL-PROJECT-RESOLVE, E-PENPAL-BREADCRUMB, E-PENPAL-WORKTREE-DROPDOWN,
// E-PENPAL-SOURCE-SECTIONS, E-PENPAL-FILE-TREE, E-PENPAL-FILE-TREE-ITEM:
// project view sidebar with breadcrumb, worktree dropdown, source file trees.
// E-PENPAL-REVIEW-COUNT: refreshReviewCount on SSE events, displayed as "In Review (count)".
// E-PENPAL-EXTERNAL-LINKS: handleAppClick intercepts external links for Tauri shell.
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
  const isFilePage = location.pathname.startsWith('/file/');

  // E-PENPAL-PROJECT-RESOLVE: detect active project from URL path.
  const pathAfterPrefix = location.pathname.match(/^\/(project|file)\/(.+)/)?.[2] || '';
  const { activeProject, activeWorktree } = useMemo(() => {
    if (!pathAfterPrefix) return { activeProject: null, activeWorktree: '' };
    const sorted = [...projects].sort((a, b) => b.qualifiedName.length - a.qualifiedName.length);
    for (const p of sorted) {
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
    const parsed = parseProjectWorktree(pathAfterPrefix.split('/').slice(0, 2).join('/'));
    const fallbackProject = projects.find((p) => p.qualifiedName === parsed.project) || null;
    return { activeProject: fallbackProject, activeWorktree: parsed.worktree };
  }, [pathAfterPrefix, projects]);
  const isProjectMode = !!pathAfterPrefix;

  // Add modal state
  const [showAddModal, setShowAddModal] = useState(false);
  const [addPath, setAddPath] = useState('');
  const [addError, setAddError] = useState('');
  const [addLoading, setAddLoading] = useState(false);

  // Install tools modal state
  const [showInstallModal, setShowInstallModal] = useState(false);

  // Find bar state
  const [showFindBar, setShowFindBar] = useState(false);

  // Context menu state (right-click)
  // E-PENPAL-CONTEXT-MENU
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; items: ContextMenuItem[] } | null>(null);

  // File selection state (shift-click)
  // E-PENPAL-BATCH-OPS
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const lastClickedFile = useRef<string | null>(null);

  // Delete confirmation modal
  const [deleteFiles, setDeleteFiles] = useState<{ project: string; path: string }[]>([]);
  const [deleting, setDeleting] = useState(false);

  // E-PENPAL-SIDEBAR-RESIZE: resizable left sidebar state
  const [sidebarWidth, setSidebarWidth] = useState(() => {
    const saved = localStorage.getItem('sidebarWidth');
    return saved ? parseInt(saved, 10) : 240;
  });
  const sidebarResizing = useRef(false);
  const sidebarResizeStartX = useRef(0);
  const sidebarResizeStartWidth = useRef(0);
  const sidebarWidthRef = useRef(sidebarWidth);

  useEffect(() => {
    sidebarWidthRef.current = sidebarWidth;
  }, [sidebarWidth]);

  const handleSidebarResizeMouseDown = useCallback((e: React.MouseEvent) => {
    sidebarResizing.current = true;
    sidebarResizeStartX.current = e.clientX;
    sidebarResizeStartWidth.current = sidebarWidthRef.current;
    e.preventDefault();

    const onMouseMove = (ev: MouseEvent) => {
      if (ev.buttons === 0) {
        cleanup();
        return;
      }
      const delta = ev.clientX - sidebarResizeStartX.current;
      const newWidth = Math.min(Math.max(sidebarResizeStartWidth.current + delta, 200), 700);
      setSidebarWidth(newWidth);
      sidebarWidthRef.current = newWidth;
    };
    const onMouseUp = () => {
      cleanup();
    };
    const cleanup = () => {
      sidebarResizing.current = false;
      localStorage.setItem('sidebarWidth', String(sidebarWidthRef.current));
      document.removeEventListener('mousemove', onMouseMove);
      document.removeEventListener('mouseup', onMouseUp);
    };
    document.addEventListener('mousemove', onMouseMove);
    document.addEventListener('mouseup', onMouseUp);
  }, []);

  // Home tree expansion state
  const [expandedWorkspaces, setExpandedWorkspaces] = useState<Set<string>>(new Set());
  const [expandedWorktreeProjects, setExpandedWorktreeProjects] = useState<Set<string>>(new Set());

  // Project sidebar state
  const [projectFiles, setProjectFiles] = useState<APIFileGroupView[]>([]);
  const [projectReviews, setProjectReviews] = useState<Record<string, APIFileInReview>>({});
  const [expandedSources, setExpandedSources] = useState<Set<string>>(new Set());
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [showWorktreeDropdown, setShowWorktreeDropdown] = useState(false);
  const worktreeDropdownRef = useRef<HTMLDivElement>(null);

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

  // Close worktree dropdown on outside click
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (worktreeDropdownRef.current && !worktreeDropdownRef.current.contains(e.target as Node)) {
        setShowWorktreeDropdown(false);
      }
    }
    if (showWorktreeDropdown) {
      document.addEventListener('mousedown', handleClick);
      return () => document.removeEventListener('mousedown', handleClick);
    }
  }, [showWorktreeDropdown]);

  // Clear selection when project changes
  useEffect(() => {
    setSelected(new Set());
    lastClickedFile.current = null;
  }, [activeProject?.qualifiedName]);

  // Fetch project files and reviews when active project changes
  useEffect(() => {
    if (!activeProject) {
      setProjectFiles([]);
      setProjectReviews({});
      return;
    }
    const qn = activeProject.qualifiedName;
    const wt = activeWorktree || undefined;
    api.getProjectFiles(qn, wt).then(setProjectFiles).catch(() => setProjectFiles([]));
    api.getReviews(qn, wt).then((reviews) => {
      const map: Record<string, APIFileInReview> = {};
      for (const r of reviews) map[r.filePath] = r;
      setProjectReviews(map);
    }).catch(() => setProjectReviews({}));
  }, [activeProject?.qualifiedName, activeWorktree]); // eslint-disable-line react-hooks/exhaustive-deps

  // Refresh project files on SSE file/comment events
  useSSE(
    useCallback(
      (event: SSEEvent) => {
        if (!activeProject) return;
        if (event.type === 'files' && event.project === activeProject.qualifiedName) {
          const wt = activeWorktree || undefined;
          api.getProjectFiles(activeProject.qualifiedName, wt).then(setProjectFiles).catch(() => {});
        }
        if (event.type === 'comments' && event.project === activeProject.qualifiedName) {
          const wt = activeWorktree || undefined;
          api.getReviews(activeProject.qualifiedName, wt).then((reviews) => {
            const map: Record<string, APIFileInReview> = {};
            for (const r of reviews) map[r.filePath] = r;
            setProjectReviews(map);
          }).catch(() => {});
        }
      },
      [activeProject?.qualifiedName, activeWorktree],
    ),
    useCallback(() => {}, []),
  );

  const { sortOrder, setSortOrder, showEmpty, setShowEmpty } = useProjectSort();

  // E-PENPAL-VIEW-OPTIONS: view options panel state
  const [showViewOptions, setShowViewOptions] = useState(false);
  const viewOptionsPanelRef = useRef<HTMLDivElement>(null);

  // Close view options panel on outside click
  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (viewOptionsPanelRef.current && !viewOptionsPanelRef.current.contains(e.target as Node)) {
        setShowViewOptions(false);
      }
    }
    if (showViewOptions) {
      document.addEventListener('mousedown', handleClick);
      return () => document.removeEventListener('mousedown', handleClick);
    }
  }, [showViewOptions]);

  // Group projects by workspace
  const workspaceProjects = useMemo(
    () => projects.filter((p) => p.origin === 'workspace'),
    [projects],
  );
  // E-PENPAL-SORT: shared comparator — empty projects last, then alpha or API order.
  const projectSort = useCallback((a: APIProject, b: APIProject) => {
    if ((a.fileCount > 0) !== (b.fileCount > 0)) return b.fileCount > 0 ? 1 : -1;
    if (sortOrder === 'alpha') return a.name.localeCompare(b.name);
    return 0;
  }, [sortOrder]);

  // E-PENPAL-VIEW-OPTIONS: filter empty projects when showEmpty is false
  const filterEmpty = useCallback((p: APIProject) => showEmpty || p.fileCount > 0, [showEmpty]);

  const standaloneProjects = useMemo(() => {
    return projects.filter((p) => p.origin === 'standalone').filter(filterEmpty).sort(projectSort);
  }, [projects, sortOrder, projectSort, filterEmpty]);
  const workspaces = useMemo(() => {
    const ws = [...new Set(workspaceProjects.map((p) => p.workspace))];
    ws.sort((a, b) => a.localeCompare(b));
    return ws;
  }, [workspaceProjects]);
  const sortedWorkspaceProjects = useMemo(() => {
    const map = new Map<string, APIProject[]>();
    for (const ws of workspaces) {
      map.set(ws, workspaceProjects.filter(p => p.workspace === ws).filter(filterEmpty).sort(projectSort));
    }
    return map;
  }, [workspaces, workspaceProjects, projectSort, filterEmpty]);

  // E-PENPAL-VIEW-OPTIONS: hide workspaces with no visible projects when showEmpty is false
  const visibleWorkspaces = useMemo(() => {
    return workspaces.filter(ws => (sortedWorkspaceProjects.get(ws) || []).length > 0);
  }, [workspaces, sortedWorkspaceProjects]);

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
      openTab('/', 'Home');
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
  }, [activeTabId, clearWindowFocusOnClose, closeTab, openTab, activateTab, tabs, visibleWorkspaces, standaloneProjects, goBack, goForward]);

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

  // (activeProject/activeWorktree/isProjectMode computed earlier, near line 45)

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
        setShowAddModal(false);
        setAddPath('');
        navigate('/');
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
    const wsProject = workspaceProjects.find((p) => p.workspace === ws);
    const wsPath = wsProject?.workspacePath;
    if (!wsPath) return;
    const wsProjectNames = workspaceProjects.filter(p => p.workspace === ws).map(p => p.qualifiedName);
    api.removeWorkspace(wsPath)
      .then(() => {
        refreshProjects();
        // Navigate home if viewing a project that belonged to the removed workspace
        if (wsProjectNames.some(qn => location.pathname.startsWith(`/project/${qn}`) || location.pathname.startsWith(`/file/${qn}`))) {
          navigate('/');
        }
      })
      .catch((err) => alert('Failed to remove workspace: ' + err.message));
  }

  function handleCloseStandaloneProject(p: APIProject) {
    api.closeProject(p.projectPath)
      .then(() => {
        refreshProjects();
        if (location.pathname.startsWith(`/project/${p.qualifiedName}`)) {
          navigate('/');
        }
      })
      .catch((err) => alert('Failed to close project: ' + err.message));
  }

  // E-PENPAL-SOURCE-ACTIONS, E-PENPAL-BATCH-OPS: file and source action helpers
  const qn = activeProject?.qualifiedName || '';

  const refreshFilesTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  function debouncedRefreshFiles() {
    if (!qn) return;
    clearTimeout(refreshFilesTimer.current);
    refreshFilesTimer.current = setTimeout(() => {
      api.getProjectFiles(qn, activeWorktree || undefined).then(setProjectFiles).catch(() => {});
    }, 200);
  }

  function showContextMenu(e: React.MouseEvent, items: ContextMenuItem[]) {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ x: e.clientX, y: e.clientY, items });
  }

  // File actions
  function fileContextMenu(e: React.MouseEvent, file: { path: string; sourceType?: string }, source: APIFileGroupView) {
    const items: ContextMenuItem[] = [
      { label: 'Copy markdown', onClick: () => api.getRawFile(qn, file.path).then(t => navigator.clipboard.writeText(t)).catch(() => {}) },
      { label: 'Copy relative path', onClick: () => navigator.clipboard.writeText('@' + file.path) },
      { label: 'Copy absolute path', onClick: () => navigator.clipboard.writeText((activeProject?.projectPath || '') + '/' + file.path) },
      { label: '---', onClick: () => {} },
      { label: 'Publish', onClick: () => api.publish(qn, file.path).then(d => navigator.clipboard.writeText(d.url)).catch(err => alert(err.message)) },
    ];
    if (source.sourceType === 'files') {
      items.push({ label: '---', onClick: () => {} });
      items.push({ label: 'Remove from Penpal', className: 'menu-muted', onClick: () => api.removeSource(qn, undefined, file.path).then(() => debouncedRefreshFiles()).catch(err => alert(err.message)) });
    }
    items.push({ label: '---', onClick: () => {} });
    items.push({ label: 'Delete from disk', className: 'menu-danger', onClick: () => setDeleteFiles([{ project: qn, path: file.path }]) });
    showContextMenu(e, items);
  }

  // Source actions
  function sourceContextMenu(e: React.MouseEvent, group: APIFileGroupView) {
    const items: ContextMenuItem[] = [
      { label: 'Copy relative paths', onClick: () => navigator.clipboard.writeText((group.files || []).map(f => '@' + f.path).join('\n')) },
      { label: 'Copy absolute paths', onClick: () => navigator.clipboard.writeText((group.files || []).map(f => (activeProject?.projectPath || '') + '/' + f.path).join('\n')) },
      { label: '---', onClick: () => {} },
      { label: 'Publish all', onClick: () => { Promise.allSettled((group.files || []).map(f => api.publish(qn, f.path))); } },
    ];
    if (!group.auto) {
      items.push({ label: '---', onClick: () => {} });
      items.push({ label: 'Remove from Penpal', className: 'menu-muted', onClick: () => {
        if (group.sourceType === 'files') {
          const files = group.files || [];
          if (!confirm(`Remove ${files.length} file(s) from Penpal?\nNo files will be deleted.`)) return;
          Promise.allSettled(files.map(f => api.removeSource(qn, undefined, f.path))).then(() => debouncedRefreshFiles());
        } else {
          if (!confirm(`Remove source "${group.name}" from this project?\nNo files will be deleted.`)) return;
          api.removeSource(qn, group.name).then(() => debouncedRefreshFiles()).catch(err => alert(err.message));
        }
      }});
    }
    items.push({ label: '---', onClick: () => {} });
    items.push({ label: 'Delete from disk', className: 'menu-danger', onClick: () => {
      const files = (group.files || []).map(f => ({ project: qn, path: f.path }));
      if (files.length > 0) setDeleteFiles(files);
    }});
    showContextMenu(e, items);
  }

  // Shift-click selection: toggle single file or extend range
  function handleFileClick(e: React.MouseEvent, filePath: string, allFilePaths: string[]) {
    if (e.shiftKey) {
      // Always prevent navigation and text selection on shift-click
      e.preventDefault();
      e.stopPropagation();
      if (lastClickedFile.current) {
        const startIdx = allFilePaths.indexOf(lastClickedFile.current);
        const endIdx = allFilePaths.indexOf(filePath);
        if (startIdx !== -1 && endIdx !== -1) {
          const lo = Math.min(startIdx, endIdx);
          const hi = Math.max(startIdx, endIdx);
          setSelected(prev => {
            const next = new Set(prev);
            for (let i = lo; i <= hi; i++) next.add(allFilePaths[i]);
            return next;
          });
          return;
        }
      }
      // No anchor yet — select just this file
      setSelected(prev => {
        const next = new Set(prev);
        next.add(filePath);
        return next;
      });
      lastClickedFile.current = filePath;
    } else {
      lastClickedFile.current = filePath;
    }
  }

  // Batch operations
  function getSelectedFiles() {
    const allFiles: { path: string }[] = [];
    projectFiles.forEach(g => (g.files || []).forEach(f => allFiles.push(f)));
    return allFiles.filter(f => selected.has(f.path));
  }

  function copySelectedMarkdown() {
    const files = getSelectedFiles();
    Promise.all(files.map(f => api.getRawFile(qn, f.path)))
      .then(texts => navigator.clipboard.writeText(texts.join('\n\n---\n\n')));
  }

  function copySelectedPaths() {
    navigator.clipboard.writeText(getSelectedFiles().map(f => '@' + f.path).join('\n'));
  }

  function publishSelected() {
    Promise.allSettled(getSelectedFiles().map(f => api.publish(qn, f.path)));
  }

  function deleteSelected() {
    const files = getSelectedFiles().map(f => ({ project: qn, path: f.path }));
    if (files.length > 0) setDeleteFiles(files);
  }

  function executeDelete() {
    setDeleting(true);
    Promise.allSettled(deleteFiles.map(d => api.deleteFile(d.project, d.path)))
      .then(results => {
        const failed = results.filter(r => r.status === 'rejected').length;
        setDeleteFiles([]);
        setDeleting(false);
        setSelected(new Set());
        debouncedRefreshFiles();
        if (failed > 0) alert(`${failed} file(s) failed to delete.`);
      });
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


  // Toggle helpers for tree expansion
  function toggleWorkspace(ws: string) {
    setExpandedWorkspaces(prev => {
      const next = new Set(prev);
      if (next.has(ws)) next.delete(ws); else next.add(ws);
      return next;
    });
  }
  function toggleWorktreeProject(qn: string) {
    setExpandedWorktreeProjects(prev => {
      const next = new Set(prev);
      if (next.has(qn)) next.delete(qn); else next.add(qn);
      return next;
    });
  }
  function toggleSource(name: string) {
    setExpandedSources(prev => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name); else next.add(name);
      return next;
    });
  }
  function toggleDir(key: string) {
    setExpandedDirs(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      return next;
    });
  }

  // Build file URL for a project file
  function fileUrl(file: { path: string }) {
    if (!activeProject) return '#';
    const base = `/file/${activeProject.qualifiedName}`;
    const wt = activeWorktree ? `@${activeWorktree}` : '';
    return `${base}${wt}/${file.path}`;
  }

  // Get the current file path from the URL (when on a file page)
  const currentFilePath = (() => {
    if (!isFilePage || !activeProject) return '';
    const prefix = `/file/${activeProject.qualifiedName}`;
    let rest = location.pathname.slice(prefix.length);
    // Strip @worktree prefix
    if (rest.startsWith('@')) {
      rest = rest.slice(rest.indexOf('/'));
    }
    if (rest.startsWith('/')) rest = rest.slice(1);
    return rest;
  })();

  // Build a tree structure from flat file list, then compact single-child directory chains
  function buildFileTree(files: { path: string; name: string; title?: string; fileType?: string; dir?: string }[]) {
    interface TreeNode {
      name: string;
      path: string;
      isDir: boolean;
      children: TreeNode[];
      file?: typeof files[0];
    }
    const root: TreeNode = { name: '', path: '', isDir: true, children: [] };
    for (const file of files) {
      const parts = file.path.split('/');
      let node = root;
      for (let i = 0; i < parts.length - 1; i++) {
        const dirPath = parts.slice(0, i + 1).join('/');
        let child = node.children.find(c => c.isDir && c.path === dirPath);
        if (!child) {
          child = { name: parts[i], path: dirPath, isDir: true, children: [] };
          node.children.push(child);
        }
        node = child;
      }
      node.children.push({ name: file.name, path: file.path, isDir: false, children: [], file });
    }
    // Compact single-child directory chains: a/ -> b/ -> c/ becomes a/b/c/
    function compact(node: TreeNode): TreeNode {
      node.children = node.children.map(compact);
      if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
        const child = node.children[0];
        return { ...child, name: node.name + '/' + child.name };
      }
      return node;
    }
    return compact(root);
  }

  // Flatten a file tree into visual (depth-first) order for shift-click ranges.
  function flattenTree(node: ReturnType<typeof buildFileTree>): string[] {
    const paths: string[] = [];
    for (const child of node.children) {
      if (child.isDir) {
        paths.push(...flattenTree(child));
      } else {
        paths.push(child.path);
      }
    }
    return paths;
  }

  const outletContext: LayoutContext = { setHeadings, projects };

  return (
    <div className="app" data-testid="app-layout" onClick={handleAppClick} style={{ gridTemplateColumns: `${sidebarWidth}px 4px 1fr` }}>
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
            <span className="tab-title" title={tab.title}>{tab.title}</span>
            {tabs.length > 1 && (
              <span className="tab-close" onClick={(e) => { e.stopPropagation(); closeTab(tab.id); }}>×</span>
            )}
          </button>
        ))}
        <button className="tab-bar-new" onClick={() => {
          openTab('/', 'Home');
        }} aria-label="New tab">+</button>
      </div>

      <nav className="sidebar" data-testid="sidebar">
        {isProjectMode && activeProject ? (
          <>
            {/* Breadcrumb bar */}
            <div className="breadcrumb-bar">
              <Link to="/" className="breadcrumb-home" title="Home">⌂</Link>
              <span className="sep">/</span>
              <Link to={`/project/${activeProject.qualifiedName}${activeWorktree ? `@${activeWorktree}` : ''}`} className="current">
                {activeProject.workspace ? `${activeProject.workspace} / ` : ''}{activeProject.name}
              </Link>
              {activeProject.agentConnected && <span className="agent-dot" />}
            </div>
            {/* E-PENPAL-WORKTREE-DROPDOWN: full-width worktree selector row below breadcrumb */}
            {activeProject.worktrees && activeProject.worktrees.length > 1 ? (
              <div className="worktree-selector-row" ref={worktreeDropdownRef} onClick={() => setShowWorktreeDropdown(!showWorktreeDropdown)}>
                {(() => {
                  const wt = activeProject.worktrees!.find(wt => activeWorktree ? wt.name === activeWorktree : wt.isMain);
                  const isMain = !wt || wt.isMain;
                  return isMain ? 'main repo' : (
                    <>
                      <svg className="worktree-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
                        <circle cx="6" cy="5" r="2" /><circle cx="18" cy="5" r="2" /><circle cx="18" cy="19" r="2" />
                        <path d="M8 5h8" /><path d="M8 5v8a6 6 0 0 0 6 6h2" />
                      </svg>
                      {wt!.name}
                    </>
                  );
                })()}
                {showWorktreeDropdown && (
                  <div className="worktree-dropdown-menu">
                    {activeProject.worktrees.map(wt => {
                      const isActive = wt.isMain ? !activeWorktree : activeWorktree === wt.name;
                      const url = wt.isMain
                        ? `/project/${activeProject.qualifiedName}`
                        : `/project/${activeProject.qualifiedName}@${wt.name}`;
                      return (
                        <button
                          key={wt.name}
                          className={isActive ? 'active' : ''}
                          title={wt.branch ? `branch: ${wt.branch}` : undefined}
                          onClick={(e) => {
                            e.stopPropagation();
                            setShowWorktreeDropdown(false);
                            navigate(url);
                          }}
                        >
                          {!wt.isMain && (
                            <svg className="worktree-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
                              <circle cx="6" cy="5" r="2" /><circle cx="18" cy="5" r="2" /><circle cx="18" cy="19" r="2" />
                              <path d="M8 5h8" /><path d="M8 5v8a6 6 0 0 0 6 6h2" />
                            </svg>
                          )}
                          {wt.isMain ? 'main repo' : wt.name}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            ) : (
              <div className="worktree-selector-row deemphasized">no worktrees</div>
            )}

            {isFilePage ? (
              /* File view: only show table of contents below breadcrumb */
              headings.length > 0 ? <TableOfContents headings={headings} /> : null
            ) : (
              /* Project view: show source file trees */
              <>
                {/* E-PENPAL-FE-SRC-DISAMBIG: compute badge texts that appear on multiple groups */}
                {(() => {
                  const badgeCounts = new Map<string, number>();
                  for (const g of projectFiles) {
                    if (g.badgeText) {
                      badgeCounts.set(g.badgeText, (badgeCounts.get(g.badgeText) || 0) + 1);
                    }
                  }
                  const duplicatedBadges = new Set<string>();
                  for (const [badge, count] of badgeCounts) {
                    if (count > 1) duplicatedBadges.add(badge);
                  }
                  return projectFiles.map((group) => {
                  const isExpanded = expandedSources.has(group.name);
                  const tree = isExpanded ? buildFileTree(group.files) : null;
                  const allFilePaths = tree ? flattenTree(tree) : [];

                  function renderTreeNode(node: ReturnType<typeof buildFileTree>, sourceKey: string): ReactNode {
                    return node.children.map(child => {
                      if (child.isDir) {
                        const dirKey = `${sourceKey}:${child.path}`;
                        const isDirExpanded = expandedDirs.has(dirKey);
                        return (
                          <div key={dirKey}>
                            <div className="tree-item" onClick={() => toggleDir(dirKey)}>
                              <span className={`chevron${isDirExpanded ? ' open' : ''}`}>▶</span>
                              <span className="label" title={child.name + '/'}>{child.name}/</span>
                            </div>
                            {isDirExpanded && (
                              <div className="tree-children">
                                {renderTreeNode(child, sourceKey)}
                              </div>
                            )}
                          </div>
                        );
                      }
                      const url = fileUrl(child);
                      const isActive = currentFilePath === child.path;
                      const inReview = !!projectReviews[child.path];
                      const isSelected = selected.has(child.path);
                      return (
                        <Link
                          key={child.path}
                          to={url}
                          className={`tree-item${isActive ? ' active' : ''}${isSelected ? ' selected' : ''}`}
                          onClick={(e) => handleFileClick(e, child.path, allFilePaths)}
                          onContextMenu={(e) => fileContextMenu(e, { path: child.path, sourceType: group.sourceType }, group)}
                        >
                          <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                          <span className="label" title={child.file?.title || child.name}>{child.file?.title || child.name}</span>
                          {child.file?.fileType && child.file.fileType !== 'other' && <span className={`badge-file-type badge-file-type-${child.file.fileType}`}>{child.file.fileType}</span>}
                          {inReview && <span className="badge-review">in review</span>}
                        </Link>
                      );
                    });
                  }

                  const isEmpty = !group.files || group.files.length === 0;
                  const isVirtual = group.source === '__all_markdown__';
                  const displayName = isEmpty && isVirtual ? 'No Markdown Found' : group.name;

                  return (
                    <div key={group.name}>
                      <div
                        className={`source-header${isEmpty && isVirtual ? ' deemphasized' : ''}`}
                        onClick={isEmpty ? undefined : () => toggleSource(group.name)}
                        onContextMenu={isEmpty ? undefined : (e) => sourceContextMenu(e, group)}
                      >
                        {isEmpty ? (
                          <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                        ) : (
                          <span className={`chevron${isExpanded ? ' open' : ''}`}>▶</span>
                        )}
                        {group.badgeText ? (
                          <span
                            className="badge-source"
                            style={{ background: group.badgeBg, color: group.badgeColor }}
                          >
                            {group.badgeText}
                          </span>
                        ) : (
                          <span>{displayName}</span>
                        )}
                        {/* E-PENPAL-FE-SRC-DISAMBIG: show source path when badge is shared by multiple groups */}
                        {group.badgeText && duplicatedBadges.has(group.badgeText) && (
                          <span className="source-disambig" title={group.name}>{group.name}</span>
                        )}
                        {!isEmpty && <span className="source-count">{group.files.length}</span>}
                      </div>
                      {isExpanded && tree && (
                        <div className="source-body">
                          {renderTreeNode(tree, group.name)}
                        </div>
                      )}
                    </div>
                  );
                });
                })()}

                {/* Per-project In Review section */}
                {(() => {
                  const reviewFiles = Object.keys(projectReviews);
                  const isEmpty = reviewFiles.length === 0;
                  const isExpanded = expandedSources.has('__in_review__');
                  return (
                    <div>
                      <div
                        className={`source-header${isEmpty ? ' deemphasized' : ''}`}
                        onClick={isEmpty ? undefined : () => toggleSource('__in_review__')}
                      >
                        {isEmpty ? (
                          <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                        ) : (
                          <span className={`chevron${isExpanded ? ' open' : ''}`}>▶</span>
                        )}
                        <span>{isEmpty ? 'Nothing in Review' : 'In Review'}</span>
                        {!isEmpty && <span className="source-count">{reviewFiles.length}</span>}
                      </div>
                      {isExpanded && !isEmpty && (
                        <div className="source-body">
                          {reviewFiles.map(filePath => {
                            const url = fileUrl({ path: filePath });
                            const name = filePath.split('/').pop() || filePath;
                            const isActive = currentFilePath === filePath;
                            return (
                              <Link key={filePath} to={url} className={`tree-item${isActive ? ' active' : ''}`}>
                                <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                                <span className="label" title={name}>{name}</span>
                              </Link>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  );
                })()}

                {/* Per-project Recent section — currently always empty (TODO: fetch per-project recent files) */}
                <div>
                  <div className="source-header deemphasized">
                    <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                    <span>Nothing Recent</span>
                  </div>
                </div>
              </>
            )}
          </>
        ) : (
          <>
            {/* Home view sidebar */}
            {/* E-PENPAL-FE-HOME-LABEL: show "Home" label next to house icon on home screen */}
            {/* E-PENPAL-VIEW-OPTIONS: view options popover with sort order and show-empty toggle */}
            <div className="sidebar-home-header">
              <span className="home-icon">⌂</span>
              <span className="home-label">Home</span>
              <div className="view-options-wrap" ref={viewOptionsPanelRef}>
                <button
                  className="view-options-btn"
                  title="View options"
                  aria-label="View options"
                  aria-haspopup="true"
                  aria-expanded={showViewOptions}
                  onClick={() => setShowViewOptions((prev) => !prev)}
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" width="14" height="14" aria-hidden="true">
                    <line x1="4" y1="6" x2="20" y2="6" />
                    <circle cx="9" cy="6" r="2" />
                    <line x1="4" y1="12" x2="20" y2="12" />
                    <circle cx="15" cy="12" r="2" />
                    <line x1="4" y1="18" x2="20" y2="18" />
                    <circle cx="12" cy="18" r="2" />
                  </svg>
                </button>
                {showViewOptions && (
                  <div className="view-options-panel">
                    <label className="view-options-label">
                      Project order
                      <select
                        value={sortOrder}
                        onChange={(e) => setSortOrder(e.target.value as 'alpha' | 'recent')}
                      >
                        <option value="alpha">A→Z</option>
                        <option value="recent">Most Recent</option>
                      </select>
                    </label>
                    <label className="view-options-label view-options-toggle">
                      <input
                        type="checkbox"
                        checked={showEmpty}
                        onChange={(e) => setShowEmpty(e.target.checked)}
                      />
                      Show empty projects
                    </label>
                  </div>
                )}
              </div>
              <button
                title="Add workspace or project"
                onClick={() => { setShowAddModal(true); setAddPath(''); setAddError(''); }}
              >+</button>
            </div>

            {/* Workspaces */}
            {visibleWorkspaces.map(ws => {
              const isExpanded = expandedWorkspaces.has(ws);
              const wsProjects = sortedWorkspaceProjects.get(ws) || [];
              const hasAgent = wsProjects.some(p => p.agentConnected);
              return (
                <div key={ws}>
                  <div
                    className="tree-item"
                    style={{ fontWeight: 500 }}
                    onClick={() => toggleWorkspace(ws)}
                    onContextMenu={(e) => showContextMenu(e, [
                      { label: 'Remove workspace', className: 'menu-danger', onClick: () => handleRemoveWorkspace(ws) },
                    ])}
                  >
                    <span className={`chevron${isExpanded ? ' open' : ''}`}>▶</span>
                    <span className="label" title={ws}>{ws}</span>
                    {hasAgent && <span className="agent-dot" />}
                  </div>
                  {isExpanded && (
                    <div className="tree-children">
                      {wsProjects.map(p => {
                        const hasWorktrees = p.worktrees && p.worktrees.length > 1;
                        const isWtExpanded = expandedWorktreeProjects.has(p.qualifiedName);
                        return (
                          <div key={p.qualifiedName}>
                            <div
                              className={`tree-item${p.fileCount === 0 ? ' deemphasized' : ''}`}
                              onClick={() => {
                                if (hasWorktrees) {
                                  toggleWorktreeProject(p.qualifiedName);
                                } else {
                                  navigate(`/project/${p.qualifiedName}`);
                                }
                              }}
                            >
                              {hasWorktrees ? (
                                <span className={`chevron${isWtExpanded ? ' open' : ''}`}>▶</span>
                              ) : (
                                <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                              )}
                              <span className="label" title={p.branch ? `${p.name}\nbranch: ${p.branch}` : p.name}>{p.name}</span>
                              {p.agentConnected && <span className="agent-dot" />}
                            </div>
                            {hasWorktrees && isWtExpanded && (
                              <div className="tree-children">
                                {p.worktrees!.map(wt => {
                                  const url = wt.isMain
                                    ? `/project/${p.qualifiedName}`
                                    : `/project/${p.qualifiedName}@${wt.name}`;
                                  return (
                                    <Link key={wt.name} to={url} className="tree-item worktree-item" title={wt.branch ? `branch: ${wt.branch}` : undefined}>
                                      <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                                      {!wt.isMain && (
                                        <svg className="worktree-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
                                          <circle cx="6" cy="5" r="2" /><circle cx="18" cy="5" r="2" /><circle cx="18" cy="19" r="2" />
                                          <path d="M8 5h8" /><path d="M8 5v8a6 6 0 0 0 6 6h2" />
                                        </svg>
                                      )}
                                      <span className="label" title={wt.isMain ? p.name : wt.name}>{wt.isMain ? p.name : wt.name}</span>
                                    </Link>
                                  );
                                })}
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}
                </div>
              );
            })}

            {/* Divider between workspaces and standalone projects */}
            {visibleWorkspaces.length > 0 && standaloneProjects.length > 0 && (
              <div className="home-section-divider" />
            )}

            {/* Standalone projects */}
            {standaloneProjects.map(p => (
              <Link
                key={p.qualifiedName}
                to={`/project/${p.qualifiedName}`}
                className={`tree-item${p.fileCount === 0 ? ' deemphasized' : ''}`}
                onContextMenu={(e) => showContextMenu(e, [
                  { label: 'Close project', className: 'menu-muted', onClick: () => handleCloseStandaloneProject(p) },
                ])}
              >
                <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
                <span className="label" title={p.branch ? `${p.name}\nbranch: ${p.branch}` : p.name}>{p.name}</span>
                {p.agentConnected && <span className="agent-dot" />}
              </Link>
            ))}

            {/* Divider before global nav */}
            {(visibleWorkspaces.length > 0 || standaloneProjects.length > 0) && (
              <div className="home-section-divider" />
            )}

            {/* Global In Review */}
            <NavLink
              to="/in-review"
              className={({ isActive }) =>
                `tree-item${isActive ? ' active' : ''}${reviewCount === 0 ? ' deemphasized' : ''}`
              }
            >
              <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
              <span className="label">In Review</span>
              {reviewCount > 0 && <span className="source-count">{reviewCount}</span>}
            </NavLink>

            {/* Global Recent */}
            <NavLink
              to="/recent"
              className={({ isActive }) => `tree-item${isActive ? ' active' : ''}`}
            >
              <span className="chevron" style={{ visibility: 'hidden' }}>▶</span>
              <span className="label">Recent</span>
            </NavLink>
          </>
        )}
      </nav>

      {/* E-PENPAL-SIDEBAR-RESIZE: drag handle for left sidebar resizing */}
      <div className="sidebar-resize-handle" data-testid="sidebar-resize-handle" onMouseDown={handleSidebarResizeMouseDown} />

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

      {/* Delete file confirmation modal */}
      <div className={`modal-overlay${deleteFiles.length > 0 ? ' open' : ''}`} onClick={() => setDeleteFiles([])}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <h3>Delete file{deleteFiles.length !== 1 ? 's' : ''}?</h3>
          <p>
            This will permanently delete{' '}
            <strong>{deleteFiles.length === 1 ? deleteFiles[0]?.path.split('/').pop() : `${deleteFiles.length} files`}</strong>{' '}
            from the filesystem. This cannot be undone.
          </p>
          <div className="modal-actions">
            <button className="btn-cancel" onClick={() => setDeleteFiles([])}>Cancel</button>
            <button className="btn-delete" onClick={executeDelete} disabled={deleting}>
              {deleting ? 'Deleting...' : 'Delete from disk'}
            </button>
          </div>
        </div>
      </div>

      {/* Selection bar */}
      <div className={`selection-bar${selected.size > 0 ? ' visible' : ''}`}>
        <span className="count">{selected.size} file{selected.size !== 1 ? 's' : ''} selected</span>
        <button onClick={copySelectedMarkdown}>Copy markdown</button>
        <button onClick={copySelectedPaths}>Copy paths</button>
        <button onClick={publishSelected}>Publish</button>
        <button className="danger-btn" onClick={deleteSelected}>Delete</button>
        <button className="clear-btn" onClick={() => setSelected(new Set())}>Clear</button>
      </div>

      {/* Right-click context menu */}
      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          items={contextMenu.items}
          onClose={() => setContextMenu(null)}
        />
      )}
    </div>
  );
}
