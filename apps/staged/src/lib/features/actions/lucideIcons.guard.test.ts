import { describe, expect, it } from 'vitest';

// The bundle graph this guard protects is wider than apps/staged/src: the
// workspace packages staged depends on export raw sources, so their .ts and
// .svelte files compile into the same Vite bundle.
const appSourceFiles = import.meta.glob('/src/**/*.{ts,svelte}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const packageSourceFiles = import.meta.glob('../../../../../../packages/*/src/**/*.{ts,svelte}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** App keys are vite-root absolute (`/src/...`); package keys are relative to this file. */
function toRepoRelativePath(path: string): string {
  if (path.startsWith('/')) {
    return `apps/staged${path}`;
  }
  return path.replace(/^(?:\.\.\/)+/, '');
}

const bundledSourceFiles = new Map(
  [...Object.entries(appSourceFiles), ...Object.entries(packageSourceFiles)].map(
    ([path, source]) => [toRepoRelativePath(path), source]
  )
);

const allowedFullMapFiles = new Set([
  'apps/staged/src/lib/features/actions/lucideIcons.ts',
  'apps/staged/src/lib/features/actions/lucideIcons.guard.test.ts',
]);

const bannedLucideEntrypointPattern =
  /(["'`])@lucide\/svelte(?:\/icons(?:\/index(?:\.[a-z]+)?)?)?(?:\?[^"'`]*)?\1/;

function findBannedLucideEntrypointImports(): string[] {
  return [...bundledSourceFiles]
    .filter(
      ([path, source]) =>
        !allowedFullMapFiles.has(path) && bannedLucideEntrypointPattern.test(source)
    )
    .map(([path]) => path)
    .sort();
}

describe('Lucide icon imports', () => {
  it('scans both the app sources and the workspace package sources', () => {
    // A silently empty glob (a directory rename, a wrong `../` count) would
    // hollow out this guard without failing it, so assert coverage directly.
    expect(bundledSourceFiles.has('apps/staged/src/lib/features/actions/lucideIcons.ts')).toBe(
      true
    );
    expect(
      bundledSourceFiles.has('packages/diff-viewer/src/lib/components/DiffViewer.svelte')
    ).toBe(true);
  });

  it('keeps barrel and full-map imports out of the bundled source graph', () => {
    const violations = findBannedLucideEntrypointImports();

    expect(
      violations,
      [
        'Do not import Lucide through the barrel or full icon map anywhere in',
        "staged's bundle graph (apps/staged/src plus the workspace packages it",
        'pulls in). Use per-icon @lucide/svelte/icons/<name> imports; the full',
        'map is only reachable via loadIconMap() in lucideIcons.ts. Breaking this',
        'invariant adds ~620 kB to the main bundle.',
        `Offending files: ${violations.join(', ')}`,
      ].join(' ')
    ).toEqual([]);
  });

  it('matches the banned entrypoints without blocking per-icon imports', () => {
    expect(bannedLucideEntrypointPattern.test("import { Play } from '@lucide/svelte';")).toBe(true);
    expect(bannedLucideEntrypointPattern.test("import('@lucide/svelte')")).toBe(true);
    expect(
      bannedLucideEntrypointPattern.test("import * as icons from '@lucide/svelte/icons';")
    ).toBe(true);
    expect(bannedLucideEntrypointPattern.test("import('@lucide/svelte/icons/index')")).toBe(true);
    expect(bannedLucideEntrypointPattern.test("import('@lucide/svelte/icons/index.js')")).toBe(
      true
    );
    expect(
      bannedLucideEntrypointPattern.test("import Play from '@lucide/svelte/icons/play';")
    ).toBe(false);
  });
});
