export type SearchShortcutAction = 'find' | 'next' | 'previous';

export interface SearchShortcutTarget {
  find: () => void;
  next: () => void;
  previous: () => void;
}

const targets: Array<{ id: number; target: SearchShortcutTarget }> = [];
let nextTargetId = 1;

export function registerSearchShortcutTarget(target: SearchShortcutTarget): () => void {
  const id = nextTargetId++;
  targets.push({ id, target });

  return () => {
    const index = targets.findIndex((entry) => entry.id === id);
    if (index !== -1) targets.splice(index, 1);
  };
}

export function runSearchShortcut(action: SearchShortcutAction): boolean {
  const active = targets.at(-1);
  if (!active) return false;

  if (action === 'find') {
    active.target.find();
  } else if (action === 'next') {
    active.target.next();
  } else {
    active.target.previous();
  }

  return true;
}
