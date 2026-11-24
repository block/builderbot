/**
 * The colours of a note diff are the code diff viewer's, by its own tokens.
 *
 * Nothing here renders: jsdom resolves neither `var()` nor `::highlight()`.
 * The stylesheets are read as text and held to the tokens they name, on
 * both sides: the note diff's rules, and the rules of the viewer they are
 * meant to match. If the viewer moves to another token, the second half of
 * this file fails, and the note diff is to follow it.
 */
import { describe, expect, it } from 'vitest';

import viewerSource from '../../../../../../packages/diff-viewer/src/lib/components/DiffViewer.svelte?raw';
import appCss from '../../../app.css?raw';
import rendererSource from '../../../../../../packages/diff-viewer/src/lib/utils/connectorRendererCanvas.ts?raw';
import fileTreeSource from '../diff/DiffFileTreeSection.svelte?raw';
import connectorsSource from './NoteDiffConnectors.svelte?raw';
import gapsSource from './NoteDiffGaps.svelte?raw';
import markersSource from './NoteDiffMarkers.svelte?raw';
import noteDiffCss from './noteDiffStyles.css?raw';

/** A stylesheet's rules, comments dropped: selector → declarations. */
function rules(css: string): Map<string, Map<string, string>> {
  const result = new Map<string, Map<string, string>>();
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
  for (const [, selectors, body] of text.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const declarations = new Map<string, string>();
    for (const declaration of body.split(';')) {
      const colon = declaration.indexOf(':');
      if (colon === -1) continue;
      declarations.set(declaration.slice(0, colon).trim(), declaration.slice(colon + 1).trim());
    }
    for (const selector of selectors.split(',')) {
      const key = selector.replace(/\s+/g, ' ').trim();
      result.set(key, new Map([...(result.get(key) ?? []), ...declarations]));
    }
  }
  return result;
}

/** The `<style>` block of a Svelte component. */
function styleOf(source: string): string {
  return /<style[^>]*>([\s\S]*)<\/style>/.exec(source)?.[1] ?? '';
}

/** Every custom property a stylesheet reads. */
function tokensRead(css: string): Set<string> {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, '');
  return new Set([...text.matchAll(/var\(\s*(--[\w-]+)/g)].map(([, name]) => name));
}

/** A colour written out, or mixed, rather than taken from a token. */
const LITERAL_COLOUR = /#[0-9a-f]{3,8}\b|\b(?:rgba?|hsla?|oklch|oklab|lab|lch|color-mix|color)\(/i;

const note = rules(noteDiffCss);
const viewer = rules(styleOf(viewerSource));
const markers = rules(styleOf(markersSource));
const gaps = rules(styleOf(gapsSource));
const connectors = rules(styleOf(connectorsSource));

describe('the code diff viewer', () => {
  it('paints a removed line, an added line and a modified line from the diff tokens', () => {
    expect(viewer.get('.before-pane .line.content-changed')?.get('background-color')).toBe(
      'var(--diff-removed-bg)'
    );
    expect(viewer.get('.after-pane .line.content-changed')?.get('background-color')).toBe(
      'var(--diff-added-bg)'
    );
    // One rule for both panes: a modified line is amber on either side.
    expect(viewer.get('.line.diff-modified')?.get('background-color')).toBe(
      'var(--diff-modified-bg)'
    );
  });

  it('paints the changed characters of a modified line one colour on both sides', () => {
    expect(viewer.get('.char-changed')?.get('background-color')).toBe(
      'var(--diff-modified-inline-bg)'
    );
  });

  it('fills a spine connector with the neutral changed tint and lines it with the range border', () => {
    expect(rendererSource).toMatch(/fillColor: getCssVar\('--diff-changed-bg',/);
    expect(rendererSource).toMatch(/strokeColor: getCssVar\('--diff-range-border',/);
    // One paint for every connector at rest: the renderer has no colour per kind.
    expect(rendererSource).not.toMatch(/--diff-(?:added|removed|modified)-/);
  });

  it('marks a file’s added and removed lines with the status accents, in the diff’s file list', () => {
    const tree = rules(styleOf(fileTreeSource));
    expect(tree.get('.change-segment-added')?.get('background')).toBe('var(--status-added)');
    expect(tree.get('.change-segment-deleted')?.get('background')).toBe('var(--status-deleted)');
  });
});

describe('a note diff’s block tints', () => {
  it.each([
    ['added', '--diff-added-bg', '--diff-added-bg-emphasis'],
    ['removed', '--diff-removed-bg', '--diff-removed-bg-emphasis'],
    ['modified', '--diff-modified-bg', '--diff-modified-bg-emphasis'],
  ])('tint a block that was %s with the viewer’s token, and its emphasis', (kind, rest, strong) => {
    const rule = note.get(`[data-note-diff='${kind}']`);
    expect(rule?.get('--note-diff-rest')).toBe(`var(${rest})`);
    expect(rule?.get('--note-diff-strong')).toBe(`var(${strong})`);
  });

  it('use the viewer’s own tint at rest and the emphasis for the change pointed at', () => {
    expect(note.get('.note-diff-all [data-note-diff]')?.get('--note-diff-tint')).toBe(
      'var(--note-diff-rest)'
    );
    for (const selector of [
      '[data-note-diff][data-note-diff-active]',
      '.note-diff-all [data-note-diff][data-note-diff-active]',
    ]) {
      expect(note.get(selector)?.get('--note-diff-tint')).toBe('var(--note-diff-strong)');
    }
  });

  it('paint that one tint, as the fill and as the outline around it', () => {
    const painted = note.get('.note-diff-all [data-note-diff]');
    expect(painted?.get('box-shadow')).toBe('inset 0 0 0 100vmax var(--note-diff-tint)');
    expect(painted?.get('outline')).toBe('3px solid var(--note-diff-tint)');
  });
});

describe('a note diff’s changed words', () => {
  it('are the viewer’s inline colour in the note and in the previous version', () => {
    expect(note.get('::highlight(note-diff-added)')?.get('background-color')).toBe(
      'var(--diff-modified-inline-bg)'
    );
    expect(note.get('::highlight(note-diff-removed)')?.get('background-color')).toBe(
      'var(--diff-modified-inline-bg)'
    );
  });
});

describe('a note diff’s solid colours', () => {
  // The colour each tint is made of. For a modified change that is the
  // viewer's amber and not `--status-modified`, which on the light theme is
  // a dark gold: beside the amber tint it reads as brown.
  const ACCENTS = [
    ['added', '--status-added', '--diff-added-bg-emphasis'],
    ['removed', '--status-deleted', '--diff-removed-bg-emphasis'],
    ['modified', '--diff-modified-accent', '--diff-modified-inline-bg'],
  ] as const;

  it.each(ACCENTS)('edge a changed figure that was %s', (kind, accent) => {
    expect(note.get(`[data-note-diff='${kind}']`)?.get('--note-diff-edge')).toBe(`var(${accent})`);
  });

  it.each(ACCENTS)(
    'fill the marker of a change that was %s, with a tint for its halo',
    (kind, accent, halo) => {
      const rule = markers.get(`.note-change-marker[data-kind='${kind}']`);
      expect(rule?.get('--marker-color')).toBe(`var(${accent})`);
      expect(rule?.get('--marker-halo')).toBe(`var(${halo})`);
    }
  );

  it('show a marker at full strength, and the one pointed at by its halo', () => {
    const marker = markers.get('.note-change-marker');
    expect(marker?.get('background-color')).toBe('var(--marker-color)');
    expect(marker?.has('opacity')).toBe(false);
    expect(markers.get('.note-change-marker.is-pointed')?.get('box-shadow')).toBe(
      '0 0 0 2px var(--marker-halo)'
    );
  });

  it('give the halo to the marker whose change is lit, not to one the pointer is merely over', () => {
    // A marker the text scrolled under a pointer at rest matches `:hover`,
    // and its change is not lit (noteDiffPointer.ts).
    const haloed = [...markers]
      .filter(([, declarations]) => declarations.has('box-shadow'))
      .map(([selector]) => selector);
    expect(haloed).toEqual(['.note-change-marker.is-pointed']);
    expect([...markers.keys()].filter((selector) => selector.includes(':hover'))).toEqual([]);
  });

  it.each(ACCENTS.slice(0, 2))(
    'draw the gap line of text that was %s solid when pointed at, and in the tint otherwise',
    (kind, accent, faint) => {
      const rule = gaps.get(`.note-diff-gap[data-kind='${kind}']`);
      expect(rule?.get('--gap-color')).toBe(`var(${accent})`);
      expect(rule?.get('--gap-faint')).toBe(`var(${faint})`);
      expect(gaps.get('.note-diff-gap')?.get('background-color')).toBe('var(--gap-faint)');
      expect(gaps.get('.note-diff-gap.is-active')?.get('background-color')).toBe(
        'var(--gap-color)'
      );
      expect(gaps.get('.note-diff-gap')?.has('opacity')).toBe(false);
    }
  );
});

describe('a note diff’s connectors', () => {
  it('are the viewer’s connectors at rest: its neutral fill and its range border', () => {
    expect(connectors.get('.connector-fill')?.get('fill')).toBe(
      'var(--connector-fill, var(--diff-changed-bg))'
    );
    const stroke = connectors.get('.connector-stroke');
    expect(stroke?.get('stroke')).toBe('var(--connector-stroke, var(--diff-range-border))');
    expect(stroke?.get('stroke-width')).toBe('1px');
    expect(stroke?.get('fill')).toBe('none');
  });

  it.each([
    ['added', '--diff-added-bg-emphasis', '--status-added'],
    ['removed', '--diff-removed-bg-emphasis', '--status-deleted'],
    ['modified', '--diff-modified-bg-emphasis', '--diff-modified-accent'],
  ])(
    'paint the connector of a pointed change that was %s as its blocks and its marker',
    (kind, tint, accent) => {
      const rule = connectors.get(`.note-diff-connector.is-pointed[data-kind='${kind}']`);
      expect(rule?.get('--connector-fill')).toBe(`var(${tint})`);
      expect(rule?.get('--connector-stroke')).toBe(`var(${accent})`);
      // The same tint the change's blocks are lit with, and its marker's colour.
      expect(note.get(`[data-note-diff='${kind}']`)?.get('--note-diff-strong')).toBe(
        `var(${tint})`
      );
      expect(markers.get(`.note-change-marker[data-kind='${kind}']`)?.get('--marker-color')).toBe(
        `var(${accent})`
      );
    }
  );

  it('colour only the change being pointed at', () => {
    const coloured = [...connectors.keys()].filter((selector) =>
      [...(connectors.get(selector)?.keys() ?? [])].some((property) =>
        property.startsWith('--connector-')
      )
    );
    expect(coloured).toHaveLength(3);
    expect(coloured.every((selector) => selector.includes('.is-pointed'))).toBe(true);
  });

  it('read no token but the viewer’s connector paint and the pointed change’s colours', () => {
    const own = [...tokensRead(styleOf(connectorsSource))].filter(
      (name) => !name.startsWith('--connector-')
    );
    expect(own.sort()).toEqual([
      '--diff-added-bg-emphasis',
      '--diff-changed-bg',
      '--diff-modified-accent',
      '--diff-modified-bg-emphasis',
      '--diff-range-border',
      '--diff-removed-bg-emphasis',
      '--status-added',
      '--status-deleted',
    ]);
  });
});

describe('the note diff’s stylesheets', () => {
  it('read no colour token but the viewer’s tints and the colours they are made of', () => {
    const own = [...tokensRead(noteDiffCss)].filter((name) => !name.startsWith('--note-diff-'));
    expect(own.sort()).toEqual([
      '--diff-added-bg',
      '--diff-added-bg-emphasis',
      '--diff-modified-accent',
      '--diff-modified-bg',
      '--diff-modified-bg-emphasis',
      '--diff-modified-inline-bg',
      '--diff-removed-bg',
      '--diff-removed-bg-emphasis',
      '--status-added',
      '--status-deleted',
    ]);
  });

  it('take the solid colours for edges only, never for a tint', () => {
    for (const [selector, declarations] of note) {
      for (const [property, value] of declarations) {
        if (!/--status-|--diff-modified-accent/.test(value)) continue;
        expect([selector, property]).toEqual([selector, '--note-diff-edge']);
      }
    }
  });

  it('never use --status-modified, in any of them', () => {
    for (const css of [
      noteDiffCss,
      styleOf(markersSource),
      styleOf(gapsSource),
      styleOf(connectorsSource),
    ]) {
      expect(tokensRead(css)).not.toContain('--status-modified');
    }
  });

  it.each([
    ['noteDiffStyles.css', noteDiffCss],
    ['NoteDiffMarkers.svelte', styleOf(markersSource)],
    ['NoteDiffGaps.svelte', styleOf(gapsSource)],
    ['NoteDiffConnectors.svelte', styleOf(connectorsSource)],
  ])('write out or mix no colour of their own: %s', (_name, css) => {
    expect(css.replace(/\/\*[\s\S]*?\*\//g, '')).not.toMatch(LITERAL_COLOUR);
  });
});

describe('the tokens', () => {
  const root = rules(appCss).get(':root');
  const alpha = (name: string) => {
    const [r, g, b, a] = (root?.get(name) ?? '').match(/[\d.]+/g)?.map(Number) ?? [];
    return { rgb: [r, g, b], a };
  };

  it('all have a value before the theme is applied', () => {
    const read = [
      ...[...tokensRead(noteDiffCss)].filter((n) => !n.startsWith('--note-diff-')),
      ...[...tokensRead(styleOf(connectorsSource))].filter((n) => !n.startsWith('--connector-')),
    ];
    for (const name of read) {
      expect([name, root?.has(name)]).toEqual([name, true]);
    }
  });

  it('make the modified accent the amber of the modified tints, opaque', () => {
    const [r, g, b, a] =
      (root?.get('--diff-modified-accent') ?? '').match(/[\d.]+/g)?.map(Number) ?? [];
    expect([r, g, b]).toEqual(alpha('--diff-modified-bg').rgb);
    expect(a).toBeUndefined();
  });

  it.each(['added', 'removed', 'modified'])(
    'make the %s emphasis the same colour as its tint, and stronger',
    (kind) => {
      const rest = alpha(`--diff-${kind}-bg`);
      const strong = alpha(`--diff-${kind}-bg-emphasis`);
      expect(strong.rgb).toEqual(rest.rgb);
      expect(strong.a).toBeGreaterThan(rest.a);
    }
  );

  it('keep the modified emphasis under the inline colour, so changed words show on it', () => {
    expect(alpha('--diff-modified-inline-bg').rgb).toEqual(alpha('--diff-modified-bg').rgb);
    expect(alpha('--diff-modified-bg-emphasis').a).toBeLessThan(
      alpha('--diff-modified-inline-bg').a
    );
  });
});
