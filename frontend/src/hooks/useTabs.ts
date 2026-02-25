import { useState, useCallback, useEffect, useRef } from 'react';
import { useNavigate, useLocation, useNavigationType } from 'react-router-dom';

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
  if (path.startsWith('/search')) {
    const q = new URLSearchParams(path.split('?')[1] || '').get('q');
    return q ? `Search: ${q}` : 'Search';
  }
  if (path === '/recent' || path === '/recent/') return 'Recent';
  if (path === '/in-review' || path === '/in-review/') return 'In Review';
  if (path === '/' || path === '') return 'Home';
  return path;
}

let tabCounter = 0;
function nextTabId(): string {
  return `tab-${++tabCounter}`;
}

export function useTabs(): TabsState {
  const navigate = useNavigate();
  const location = useLocation();
  const navType = useNavigationType();
  const isInitialized = useRef(false);
  const navigating = useRef(false);
  const locationRef = useRef(location);
  locationRef.current = location;

  const [tabs, setTabs] = useState<Tab[]>(() => {
    const path = location.pathname + location.search;
    return [{ id: nextTabId(), path, title: deriveTitleFromPath(path), history: [path], historyIndex: 0 }];
  });
  const [activeTabId, setActiveTabId] = useState<string>(() => tabs[0].id);

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
