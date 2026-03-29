import { useCallback, useSyncExternalStore } from 'react';

export type ProjectSortOrder = 'alpha' | 'recent';

const SORT_KEY = 'penpal-project-sort';
const SHOW_EMPTY_KEY = 'penpal-show-empty';

let listeners: Array<() => void> = [];

function emitChange() {
  for (const l of listeners) l();
}

function subscribe(listener: () => void) {
  listeners = [...listeners, listener];
  // Cross-tab sync via storage event
  function handleStorage(e: StorageEvent) {
    if (e.key === SORT_KEY || e.key === SHOW_EMPTY_KEY) listener();
  }
  window.addEventListener('storage', handleStorage);
  return () => {
    listeners = listeners.filter((l) => l !== listener);
    window.removeEventListener('storage', handleStorage);
  };
}

function getSortSnapshot(): ProjectSortOrder {
  const stored = localStorage.getItem(SORT_KEY);
  if (stored === 'alpha' || stored === 'recent') return stored;
  return 'alpha';
}

function getShowEmptySnapshot(): boolean {
  const stored = localStorage.getItem(SHOW_EMPTY_KEY);
  if (stored === 'false') return false;
  return true; // default true
}

// E-PENPAL-SORT, E-PENPAL-VIEW-OPTIONS: useSyncExternalStore backed by localStorage with cross-tab sync for project ordering and show-empty toggle.
export function useProjectSort() {
  const sortOrder = useSyncExternalStore(subscribe, getSortSnapshot, getSortSnapshot);
  const showEmpty = useSyncExternalStore(subscribe, getShowEmptySnapshot, getShowEmptySnapshot);

  const setSortOrder = useCallback((order: ProjectSortOrder) => {
    localStorage.setItem(SORT_KEY, order);
    emitChange();
  }, []);

  const setShowEmpty = useCallback((show: boolean) => {
    localStorage.setItem(SHOW_EMPTY_KEY, show ? 'true' : 'false');
    emitChange();
  }, []);

  const toggle = useCallback(() => {
    setSortOrder(sortOrder === 'alpha' ? 'recent' : 'alpha');
  }, [sortOrder, setSortOrder]);

  return { sortOrder, setSortOrder, showEmpty, setShowEmpty, toggle };
}
