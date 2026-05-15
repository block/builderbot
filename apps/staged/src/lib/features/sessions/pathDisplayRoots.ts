import { resolvePathAliases } from '../../api/commands';

export type DisplayRootInput = string | null | undefined | readonly DisplayRootInput[];
export type PathAliasResolver = (paths: string[]) => Promise<string[]>;

const FALLBACK_ALIAS_RETRY_MS = 5_000;

type AliasCacheEntry = {
  promise: Promise<string[]>;
  expiresAt?: number;
};

const pathAliasCache = new Map<string, AliasCacheEntry>();
const rootSetCache = new Map<string, AliasCacheEntry>();

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

function cachedAliases(cache: Map<string, AliasCacheEntry>, key: string): Promise<string[]> | null {
  const cached = cache.get(key);
  if (!cached) return null;
  if (cached.expiresAt !== undefined && cached.expiresAt <= Date.now()) {
    cache.delete(key);
    return null;
  }
  return cached.promise;
}

function isFallbackOnlyAlias(root: string, aliases: string[]): boolean {
  return aliases.length === 1 && aliases[0] === root;
}

function expireFallbackAlias(
  cache: Map<string, AliasCacheEntry>,
  key: string,
  entry: AliasCacheEntry,
  shouldRetry: boolean
) {
  if (!shouldRetry) return;
  if (cache.get(key) !== entry) return;
  entry.expiresAt = Date.now() + FALLBACK_ALIAS_RETRY_MS;
}

function aliasesForRoot(root: string, resolver: PathAliasResolver): Promise<string[]> {
  const cached = cachedAliases(pathAliasCache, root);
  if (cached) return cached;

  let entry: AliasCacheEntry;
  const promise = Promise.resolve()
    .then(() => resolver([root]))
    .then((aliases) => normalizeDisplayRoots([root, aliases]))
    .catch(() => [root])
    .then((aliases) => {
      expireFallbackAlias(pathAliasCache, root, entry, isFallbackOnlyAlias(root, aliases));
      return aliases;
    });

  entry = { promise };
  pathAliasCache.set(root, entry);
  return promise;
}

export function resolveDisplayRoots(
  input: DisplayRootInput,
  resolver: PathAliasResolver = resolvePathAliases
): Promise<string[]> {
  const roots = normalizeDisplayRoots(input);
  const key = displayRootKey(roots);
  if (!key) return Promise.resolve([]);

  const cached = cachedAliases(rootSetCache, key);
  if (cached) return cached;

  let entry: AliasCacheEntry;
  const promise = Promise.all(roots.map((root) => aliasesForRoot(root, resolver))).then(
    (groups) => {
      expireFallbackAlias(
        rootSetCache,
        key,
        entry,
        groups.some((aliases, index) => isFallbackOnlyAlias(roots[index], aliases))
      );
      return normalizeDisplayRoots(groups);
    }
  );
  entry = { promise };
  rootSetCache.set(key, entry);
  return promise;
}

export function clearDisplayRootAliasCacheForTests() {
  pathAliasCache.clear();
  rootSetCache.clear();
}
