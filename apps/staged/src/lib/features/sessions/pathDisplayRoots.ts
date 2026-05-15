import { resolvePathAliases } from '../../api/commands';

export type DisplayRootInput = string | null | undefined | readonly DisplayRootInput[];
export type PathAliasResolver = (paths: string[]) => Promise<string[]>;

const pathAliasCache = new Map<string, Promise<string[]>>();
const rootSetCache = new Map<string, Promise<string[]>>();

function visitDisplayRoots(input: DisplayRootInput, roots: string[]) {
  if (typeof input === 'string' || input == null) {
    const normalized = normalizeDisplayRoot(input);
    if (normalized) {
      roots.push(normalized);
    }
    return;
  }

  for (const item of input) {
    visitDisplayRoots(item, roots);
  }
}

function dedupe(values: string[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const value of values) {
    if (seen.has(value)) continue;
    seen.add(value);
    result.push(value);
  }
  return result;
}

export function normalizeDisplayRoot(root: string | null | undefined): string | null {
  if (!root) return null;

  let normalized = root.trim();
  while (normalized.length > 1 && /[/\\]$/.test(normalized)) {
    normalized = normalized.slice(0, -1);
  }

  return normalized || null;
}

export function normalizeDisplayRoots(input: DisplayRootInput): string[] {
  const roots: string[] = [];
  visitDisplayRoots(input, roots);
  return dedupe(roots);
}

export function displayRootKey(input: DisplayRootInput): string {
  return [...normalizeDisplayRoots(input)].sort().join('\n');
}

function aliasesForRoot(root: string, resolver: PathAliasResolver): Promise<string[]> {
  const cached = pathAliasCache.get(root);
  if (cached) return cached;

  const promise = resolver([root])
    .then((aliases) => normalizeDisplayRoots([root, aliases]))
    .catch(() => [root]);

  pathAliasCache.set(root, promise);
  return promise;
}

export function resolveDisplayRoots(
  input: DisplayRootInput,
  resolver: PathAliasResolver = resolvePathAliases
): Promise<string[]> {
  const roots = normalizeDisplayRoots(input);
  const key = displayRootKey(roots);
  if (!key) return Promise.resolve([]);

  const cached = rootSetCache.get(key);
  if (cached) return cached;

  const promise = Promise.all(roots.map((root) => aliasesForRoot(root, resolver))).then((groups) =>
    normalizeDisplayRoots(groups)
  );
  rootSetCache.set(key, promise);
  return promise;
}

export function clearDisplayRootAliasCacheForTests() {
  pathAliasCache.clear();
  rootSetCache.clear();
}
