import { useCallback, useSyncExternalStore } from 'react';

export type ProjectSortOrder = 'alpha' | 'recent';

const STORAGE_KEY = 'penpal-project-sort';

let listeners: Array<() => void> = [];

function emitChange() {
  for (const l of listeners) l();
}

function subscribe(listener: () => void) {
  listeners = [...listeners, listener];
  // Cross-tab sync via storage event
  function handleStorage(e: StorageEvent) {
    if (e.key === STORAGE_KEY) listener();
  }
  window.addEventListener('storage', handleStorage);
  return () => {
    listeners = listeners.filter((l) => l !== listener);
    window.removeEventListener('storage', handleStorage);
  };
}

function getSnapshot(): ProjectSortOrder {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === 'alpha' || stored === 'recent') return stored;
  return 'alpha';
}

export function useProjectSort() {
  const sortOrder = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  const setSortOrder = useCallback((order: ProjectSortOrder) => {
    localStorage.setItem(STORAGE_KEY, order);
    emitChange();
  }, []);

  const toggle = useCallback(() => {
    setSortOrder(sortOrder === 'alpha' ? 'recent' : 'alpha');
  }, [sortOrder, setSortOrder]);

  return { sortOrder, setSortOrder, toggle };
}
