import { useState, useCallback, useEffect, useRef } from 'react';
import { useNavigate, useLocation, useNavigationType } from 'react-router-dom';
import { isDesktopApp } from '../api';

export interface Tab {
  id: string;
  path: string;
  title: string;
  history: string[];
  historyIndex: number;
}

export interface TabsState {
  tabs: Tab[];
  activeTabId: string;
  openTab: (path: string, title?: string, options?: { background?: boolean }) => void;
  closeTab: (id: string) => void;
  activateTab: (id: string) => void;
  updateActiveTab: (path: string, title?: string) => void;
  canGoBack: boolean;
  canGoForward: boolean;
  goBack: () => void;
  goForward: () => void;
}

export function deriveTitleFromPath(path: string): string {
  if (path.startsWith('/file/')) {
    const parts = path.split('/');
    return parts[parts.length - 1] || 'File';
  }
  if (path.startsWith('/project/')) {
    const parts = path.replace('/project/', '').split('/');
    return parts[parts.length - 1] || 'Project';
  }
  if (path.startsWith('/workspace/')) {
    return decodeURIComponent(path.replace('/workspace/', '')) || 'Workspace';
  }
  if (path === '/recent' || path === '/recent/') return 'Recent';
  if (path === '/in-review' || path === '/in-review/') return 'In Review';
  if (path === '/' || path === '') return 'Home';
  return path;
}

// E-PENPAL-TAB-PERSIST: use crypto.randomUUID for collision-free tab IDs across sessions.
function nextTabId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return `tab-${crypto.randomUUID()}`;
  }
  return `tab-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

// E-PENPAL-TAB-PERSIST: persist/restore tab state per window label.
interface PersistedTabState {
  version: number;
  activeTabId: string;
  tabs: Tab[];
}

let windowLabelCache: string | null = null;
let windowLabelPromise: Promise<string> | null = null;

function resolveWindowLabelSync(): string | null {
  if (windowLabelCache) return windowLabelCache;
  if (!isDesktopApp) {
    windowLabelCache = 'browser';
    return 'browser';
  }
  return null; // Desktop: must resolve async
}

async function resolveWindowLabel(): Promise<string> {
  if (windowLabelCache) return windowLabelCache;
  if (!isDesktopApp) {
    windowLabelCache = 'browser';
    return 'browser';
  }
  if (!windowLabelPromise) {
    windowLabelPromise = import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => {
        windowLabelCache = getCurrentWindow().label || 'main';
        return windowLabelCache;
      })
      .catch(() => {
        windowLabelCache = 'browser';
        return 'browser';
      });
  }
  return windowLabelPromise;
}

function tabStorageKey(label: string): string {
  return `penpal:tabs:${label}`;
}

function loadPersistedTabs(label: string): PersistedTabState | null {
  try {
    const raw = localStorage.getItem(tabStorageKey(label));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as PersistedTabState;
    if (parsed.version !== 1 || !Array.isArray(parsed.tabs) || parsed.tabs.length === 0) return null;
    return parsed;
  } catch {
    return null;
  }
}

function savePersistedTabs(label: string, tabs: Tab[], activeTabId: string): void {
  try {
    const state: PersistedTabState = { version: 1, activeTabId, tabs };
    localStorage.setItem(tabStorageKey(label), JSON.stringify(state));
  } catch {
    // localStorage full or unavailable — skip
  }
}

// E-PENPAL-TABS: per-tab history management with PUSH/REPLACE/POP navigation.
// E-PENPAL-TAB-PERSIST: restores tabs from localStorage on mount, saves on mutation.
export function useTabs(): TabsState {
  const navigate = useNavigate();
  const location = useLocation();
  const navType = useNavigationType();
  const isInitialized = useRef(false);
  const navigating = useRef(false);
  const locationRef = useRef(location);
  locationRef.current = location;
  const windowLabelRef = useRef<string | null>(resolveWindowLabelSync());

  const [tabs, setTabs] = useState<Tab[]>(() => {
    // E-PENPAL-TAB-PERSIST: try to restore from localStorage synchronously.
    // In browser mode the label is available immediately. In desktop mode
    // the label may not be available yet — the async useEffect handles that.
    const label = windowLabelRef.current;
    if (label) {
      const persisted = loadPersistedTabs(label);
      if (persisted) return persisted.tabs;
    }
    const path = location.pathname + location.search;
    return [{ id: nextTabId(), path, title: deriveTitleFromPath(path), history: [path], historyIndex: 0 }];
  });
  const [activeTabId, setActiveTabId] = useState<string>(() => {
    const label = windowLabelRef.current;
    if (label) {
      const persisted = loadPersistedTabs(label);
      if (persisted) return persisted.activeTabId;
    }
    return tabs[0].id;
  });
  const tabsRef = useRef(tabs);
  tabsRef.current = tabs;

  // E-PENPAL-TAB-PERSIST: resolve window label and restore tabs if not done synchronously.
  useEffect(() => {
    resolveWindowLabel().then(label => {
      windowLabelRef.current = label;
      // If we already restored synchronously, just save the current state
      if (windowLabelCache === label && isDesktopApp) {
        // Check if we need to do async restoration — read live state via refs
        // so we don't clobber user interactions that occurred while awaiting.
        const currentTabs = tabsRef.current;
        const persisted = loadPersistedTabs(label);
        if (persisted && currentTabs.length === 1 && currentTabs[0].history.length === 1) {
          // We have persisted state but only a default tab — restore
          setTabs(persisted.tabs);
          setActiveTabId(persisted.activeTabId);
          const activeTab = persisted.tabs.find(t => t.id === persisted.activeTabId);
          if (activeTab) {
            const currentLoc = locationRef.current;
            const currentPath = currentLoc.pathname + currentLoc.search;
            if (activeTab.path !== currentPath) {
              navigating.current = true;
              navigate(activeTab.path);
            }
          }
        }
      }
    });
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // E-PENPAL-TAB-PERSIST: save tabs to localStorage on every mutation.
  useEffect(() => {
    if (windowLabelRef.current) {
      savePersistedTabs(windowLabelRef.current, tabs, activeTabId);
    }
  }, [tabs, activeTabId]);

  // Sync active tab path when URL changes (e.g. user clicks sidebar links)
  useEffect(() => {
    if (!isInitialized.current) {
      isInitialized.current = true;
      return;
    }
    const currentPath = location.pathname + location.search;
    if (navigating.current) {
      navigating.current = false;
      // Programmatic navigation (back/forward/tab switch) — just update path and title, don't push history
      setTabs(prev => prev.map(tab =>
        tab.id === activeTabId
          ? { ...tab, path: currentPath, title: deriveTitleFromPath(currentPath) }
          : tab,
      ));
      return;
    }
    // User-initiated navigation — push or replace history
    setTabs(prev => prev.map(tab => {
      if (tab.id !== activeTabId) return tab;
      // Skip if path hasn't changed
      if (tab.history[tab.historyIndex] === currentPath) return tab;
      if (navType === 'REPLACE') {
        // Redirect — replace current entry, don't push
        const newHistory = [...tab.history];
        newHistory[tab.historyIndex] = currentPath;
        return { ...tab, path: currentPath, title: deriveTitleFromPath(currentPath), history: newHistory };
      }
      if (navType === 'POP') {
        // Browser-initiated back/forward — search outward from current index
        const title = deriveTitleFromPath(currentPath);
        for (let offset = 1; offset < tab.history.length; offset++) {
          const back = tab.historyIndex - offset;
          if (back >= 0 && tab.history[back] === currentPath) {
            return { ...tab, path: currentPath, title, historyIndex: back };
          }
          const fwd = tab.historyIndex + offset;
          if (fwd < tab.history.length && tab.history[fwd] === currentPath) {
            return { ...tab, path: currentPath, title, historyIndex: fwd };
          }
        }
        // Path not found in tab history — just update path/title
        return { ...tab, path: currentPath, title };
      }
      // PUSH — truncate forward history and push new entry
      const newHistory = [...tab.history.slice(0, tab.historyIndex + 1), currentPath];
      return {
        ...tab,
        path: currentPath,
        title: deriveTitleFromPath(currentPath),
        history: newHistory,
        historyIndex: newHistory.length - 1,
      };
    }));
  }, [location.pathname, location.search]); // eslint-disable-line react-hooks/exhaustive-deps

  const openTab = useCallback((path: string, title?: string, options?: { background?: boolean }) => {
    const id = nextTabId();
    const newTab: Tab = { id, path, title: title || deriveTitleFromPath(path), history: [path], historyIndex: 0 };
    setTabs(prev => [...prev, newTab]);
    if (!options?.background) {
      setActiveTabId(id);
      const currentPath = locationRef.current.pathname + locationRef.current.search;
      if (path !== currentPath) {
        navigating.current = true;
      }
      navigate(path);
    }
  }, [navigate]);

  const closeTab = useCallback((id: string) => {
    setTabs(prev => {
      if (prev.length <= 1) return prev;
      const idx = prev.findIndex(t => t.id === id);
      if (idx === -1) return prev;
      const next = prev.filter(t => t.id !== id);
      // If we're closing the active tab, activate a neighbor
      if (id === activeTabId) {
        const newActive = next[Math.min(idx, next.length - 1)];
        setActiveTabId(newActive.id);
        const currentPath = locationRef.current.pathname + locationRef.current.search;
        if (newActive.path !== currentPath) {
          navigating.current = true;
          navigate(newActive.path);
        }
      }
      return next;
    });
  }, [activeTabId, navigate]);

  const activateTab = useCallback((id: string) => {
    setTabs(prev => {
      const tab = prev.find(t => t.id === id);
      if (tab) {
        setActiveTabId(id);
        const currentPath = locationRef.current.pathname + locationRef.current.search;
        if (tab.path !== currentPath) {
          navigating.current = true;
          navigate(tab.path);
        }
      }
      return prev;
    });
  }, [navigate]);

  const updateActiveTab = useCallback((path: string, title?: string) => {
    setTabs(prev => prev.map(tab =>
      tab.id === activeTabId
        ? { ...tab, path, title: title || deriveTitleFromPath(path) }
        : tab,
    ));
  }, [activeTabId]);

  const activeTab = tabs.find(t => t.id === activeTabId);
  const canGoBack = !!activeTab && activeTab.historyIndex > 0;
  const canGoForward = !!activeTab && activeTab.historyIndex < activeTab.history.length - 1;

  const goBack = useCallback(() => {
    setTabs(prev => {
      const tab = prev.find(t => t.id === activeTabId);
      if (!tab || tab.historyIndex <= 0) return prev;
      const newIndex = tab.historyIndex - 1;
      const targetPath = tab.history[newIndex];
      const currentPath = locationRef.current.pathname + locationRef.current.search;
      if (targetPath !== currentPath) {
        navigating.current = true;
        navigate(targetPath);
      }
      return prev.map(t =>
        t.id === activeTabId
          ? { ...t, historyIndex: newIndex, path: targetPath, title: deriveTitleFromPath(targetPath) }
          : t,
      );
    });
  }, [activeTabId, navigate]);

  const goForward = useCallback(() => {
    setTabs(prev => {
      const tab = prev.find(t => t.id === activeTabId);
      if (!tab || tab.historyIndex >= tab.history.length - 1) return prev;
      const newIndex = tab.historyIndex + 1;
      const targetPath = tab.history[newIndex];
      const currentPath = locationRef.current.pathname + locationRef.current.search;
      if (targetPath !== currentPath) {
        navigating.current = true;
        navigate(targetPath);
      }
      return prev.map(t =>
        t.id === activeTabId
          ? { ...t, historyIndex: newIndex, path: targetPath, title: deriveTitleFromPath(targetPath) }
          : t,
      );
    });
  }, [activeTabId, navigate]);

  return { tabs, activeTabId, openTab, closeTab, activateTab, updateActiveTab, canGoBack, canGoForward, goBack, goForward };
}
