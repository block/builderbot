import { describe, expect, it } from 'vitest';
import { cn } from '../../utils';

// Input's text is `text-sm`, and `text-base` in the mobile layout
// (`mobile:`). A caller's font size merges over the first only: an
// unprefixed `text-base` would make the input large on desktop too, and an
// unprefixed `text-sm` leaves it large on mobile. Each caller's effective
// size in both layouts is checked here, merged the way the component merges
// it.

const sources = import.meta.glob('/src/**/*.svelte', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

type Sizes = { mobile: string; desktop: string };

const DEFAULT: Sizes = { mobile: 'text-base', desktop: 'text-sm' };

// Callers that set their own size in both layouts: the inline search fields.
const OWN_SIZE: Record<string, Sizes> = {
  '/src/lib/features/projects/RepoSearchInput.svelte': {
    mobile: 'text-base',
    desktop: 'text-base',
  },
  '/src/lib/shared/InContentSearch.svelte': { mobile: 'text-base', desktop: 'text-base' },
  '/src/lib/shared/SearchField.svelte': { mobile: 'text-sm', desktop: 'text-sm' },
};

/**
 * A font size is whatever tailwind-merge treats as one: merged after
 * `text-sm`, it replaces it. That takes in the arbitrary sizes
 * (`text-[13px]`, `text-[length:…]`) and leaves out a bare
 * `text-[var(--x)]`, which it reads as a colour.
 */
const isFontSize = (token: string) => cn('text-sm', token) === token;

function baseClasses(): string {
  const source = sources['/src/lib/components/ui/input/input.svelte'];
  const bases = [...source.matchAll(/class=\{cn\(\s*'([^']+)',\s*className\s*\)\}/g)].map(
    (m) => m[1]
  );
  // The file and text branches share one class list.
  expect(bases).toHaveLength(2);
  expect(bases[0]).toBe(bases[1]);
  return bases[0];
}

/** Each `<Input …>` tag's source, up to its closing `>`. */
function inputTags(source: string): string[] {
  const tags: string[] = [];
  for (const match of source.matchAll(/<Input[\s/>]/g)) {
    let depth = 0;
    let quoted = false;
    for (let i = match.index + 1; i < source.length; i++) {
      const ch = source[i];
      if (quoted) {
        if (ch === '"') quoted = false;
      } else if (ch === '{') depth++;
      else if (ch === '}') depth--;
      else if (depth === 0 && ch === '"') quoted = true;
      else if (depth === 0 && ch === '>') {
        tags.push(source.slice(match.index, i + 1));
        break;
      }
    }
  }
  return tags;
}

function effectiveSizes(classes: string): Sizes {
  const tokens = classes.split(/\s+/);
  const desktop = tokens.filter(isFontSize).at(-1) ?? 'inherit';
  const mobile =
    tokens
      .filter((t) => t.startsWith('mobile:') && isFontSize(t.slice('mobile:'.length)))
      .at(-1)
      ?.slice('mobile:'.length) ?? desktop;
  return { mobile, desktop };
}

describe('Input font size', () => {
  it('is text-sm, and text-base in the mobile layout, unless a caller sizes both', () => {
    const base = baseClasses();
    expect(effectiveSizes(cn(base))).toEqual(DEFAULT);

    let callers = 0;
    for (const [file, source] of Object.entries(sources)) {
      for (const tag of inputTags(source)) {
        callers++;
        expect(tag, `${file}: give Input a static class`).not.toMatch(/\sclass=\{/);
        const className = tag.match(/\sclass="([^"]*)"/)?.[1] ?? '';
        expect(effectiveSizes(cn(base, className)), `${file}: ${tag}`).toEqual(
          OWN_SIZE[file] ?? DEFAULT
        );
      }
    }
    // A silently empty scan would pass with nothing checked.
    expect(callers).toBeGreaterThan(10);
    for (const file of Object.keys(OWN_SIZE)) expect(sources[file], file).toMatch(/<Input\b/);
  });

  it('flags a caller whose own size overrides only one layout', () => {
    const base = baseClasses();
    expect(effectiveSizes(cn(base, 'px-3.5 text-base'))).toEqual({
      mobile: 'text-base',
      desktop: 'text-base',
    });
    expect(effectiveSizes(cn(base, 'px-0 text-sm'))).toEqual({
      mobile: 'text-base',
      desktop: 'text-sm',
    });
    expect(effectiveSizes(cn(base, 'text-[13px]'))).toEqual({
      mobile: 'text-base',
      desktop: 'text-[13px]',
    });
    expect(effectiveSizes(cn(base, 'text-[length:var(--size-xs)]'))).toEqual({
      mobile: 'text-base',
      desktop: 'text-[length:var(--size-xs)]',
    });
  });

  it('reads a bare var() colour as no size at all', () => {
    expect(effectiveSizes(cn(baseClasses(), 'text-[var(--text-muted)]'))).toEqual(DEFAULT);
  });
});
