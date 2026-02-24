import { useState, useCallback, useEffect, useRef } from 'react';
import { useNavigate, useLocation } from 'react-router-dom';

export interface Tab {
  id: string;
  path: string;
  title: string;
}

export interface TabsState {
  tabs: Tab[];
  activeTabId: string;
  openTab: (path: string, title?: string, options?: { background?: boolean }) => void;
  closeTab: (id: string) => void;
  activateTab: (id: string) => void;
  updateActiveTab: (path: string, title?: string) => void;
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
  const isInitialized = useRef(false);

  const [tabs, setTabs] = useState<Tab[]>(() => {
    const path = location.pathname + location.search;
    return [{ id: nextTabId(), path, title: deriveTitleFromPath(path) }];
  });
  const [activeTabId, setActiveTabId] = useState<string>(() => tabs[0].id);

  // Sync active tab path when URL changes (e.g. user clicks sidebar links)
  useEffect(() => {
    if (!isInitialized.current) {
      isInitialized.current = true;
      return;
    }
    const currentPath = location.pathname + location.search;
    setTabs(prev => prev.map(tab =>
      tab.id === activeTabId
        ? { ...tab, path: currentPath, title: deriveTitleFromPath(currentPath) }
        : tab,
    ));
  }, [location.pathname, location.search]); // eslint-disable-line react-hooks/exhaustive-deps

  const openTab = useCallback((path: string, title?: string, options?: { background?: boolean }) => {
    const id = nextTabId();
    const newTab: Tab = { id, path, title: title || deriveTitleFromPath(path) };
    setTabs(prev => [...prev, newTab]);
    if (!options?.background) {
      setActiveTabId(id);
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
        navigate(newActive.path);
      }
      return next;
    });
  }, [activeTabId, navigate]);

  const activateTab = useCallback((id: string) => {
    setTabs(prev => {
      const tab = prev.find(t => t.id === id);
      if (tab) {
        setActiveTabId(id);
        navigate(tab.path);
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

  return { tabs, activeTabId, openTab, closeTab, activateTab, updateActiveTab };
}
