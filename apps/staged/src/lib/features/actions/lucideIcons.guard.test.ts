import { describe, expect, it } from 'vitest';

const sourceFiles = import.meta.glob('/src/**/*.{ts,svelte}', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const allowedFullMapFiles = new Set([
  '/src/lib/features/actions/lucideIcons.ts',
  '/src/lib/features/actions/lucideIcons.guard.test.ts',
]);

const bannedLucideEntrypointPattern =
  /(["'`])@lucide\/svelte(?:\/icons(?:\/index(?:\.[a-z]+)?)?)?(?:\?[^"'`]*)?\1/;

function findBannedLucideEntrypointImports(): string[] {
  return Object.entries(sourceFiles)
    .filter(
      ([path, source]) =>
        !allowedFullMapFiles.has(path) && bannedLucideEntrypointPattern.test(source)
    )
    .map(([path]) => path.replace(/^\//, ''))
    .sort();
}

describe('Lucide icon imports', () => {
  it('keeps barrel and full-map imports out of the app source graph', () => {
    const violations = findBannedLucideEntrypointImports();

    expect(
      violations,
      [
        'Do not import Lucide through the barrel or full icon map in apps/staged/src.',
        'Use per-icon @lucide/svelte/icons/<name> imports; the full map is only',
        'reachable via loadIconMap() in lucideIcons.ts. Breaking this invariant',
        'adds ~620 kB to the main bundle.',
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
