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
import blocksSource from './NoteDiffBlocks.svelte?raw';
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
const blocks = rules(styleOf(blocksSource));
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

const KINDS = [
  ['added', '--diff-added-bg', '--diff-added-bg-emphasis'],
  ['removed', '--diff-removed-bg', '--diff-removed-bg-emphasis'],
  ['modified', '--diff-modified-bg', '--diff-modified-bg-emphasis'],
] as const;

describe('a note diff’s block tints', () => {
  it.each(KINDS)(
    'fill the rectangle behind blocks that were %s with the viewer’s token, and its emphasis',
    (kind, rest, strong) => {
      const rule = blocks.get(`.note-diff-block-part[data-kind='${kind}']`);
      expect(rule?.get('--part-tint')).toBe(`var(${rest})`);
      expect(rule?.get('--part-strong')).toBe(`var(${strong})`);
    }
  );

  it('use the viewer’s own tint at rest and the emphasis for the change pointed at', () => {
    expect(blocks.get('.note-diff-block-part')?.get('background-color')).toBe('var(--part-tint)');
    expect(
      blocks.get('.note-diff-block.is-active > .note-diff-block-part')?.get('background-color')
    ).toBe('var(--part-strong)');
  });

  it.each(KINDS)(
    'tint a block that covers the rectangle, and was %s, as the rectangle is',
    (kind, rest, strong) => {
      const rule = note.get(`[data-note-diff='${kind}']`);
      expect(rule?.get('--note-diff-rest')).toBe(`var(${rest})`);
      expect(rule?.get('--note-diff-strong')).toBe(`var(${strong})`);
    }
  );

  it('tint in place only the blocks with a background of their own, and nothing twice', () => {
    expect(note.get('.note-diff-all [data-note-diff]')?.get('--note-diff-tint')).toBe(
      'var(--note-diff-rest)'
    );
    for (const selector of [
      '[data-note-diff][data-note-diff-active]',
      '.note-diff-all [data-note-diff][data-note-diff-active]',
    ]) {
      expect(note.get(selector)?.get('--note-diff-tint')).toBe('var(--note-diff-strong)');
    }
    const painted = [...note]
      .filter(([, declarations]) => declarations.has('box-shadow'))
      .map(([selector, declarations]) => [selector, declarations.get('box-shadow')]);
    expect(painted).toEqual(
      [
        '.note-diff-all pre[data-note-diff]',
        'pre[data-note-diff][data-note-diff-active]',
        '.note-diff-all tr[data-note-diff] > th',
        'tr[data-note-diff][data-note-diff-active] > th',
        '.note-diff-all figure[data-note-diff] > .markdown-diagram-preview',
        'figure[data-note-diff][data-note-diff-active] > .markdown-diagram-preview',
      ].map((selector) => [selector, 'inset 0 0 0 100vmax var(--note-diff-tint)'])
    );
    // No block is given a background: the rectangle behind it is the tint.
    expect(
      [...note].filter(
        ([selector, declarations]) =>
          !selector.startsWith('::highlight') &&
          (declarations.has('background-color') || declarations.has('background'))
      )
    ).toEqual([]);
  });
});

describe('a note diff’s square corners', () => {
  it('round nothing and outline nothing in the note’s markup', () => {
    for (const [selector, declarations] of note) {
      for (const property of ['outline', 'border-radius', 'border', 'border-color']) {
        expect([selector, property, declarations.has(property)]).toEqual([
          selector,
          property,
          false,
        ]);
      }
    }
  });

  it('round nothing in the rectangles behind the changes', () => {
    for (const [selector, declarations] of blocks) {
      expect([selector, declarations.has('border-radius')]).toEqual([selector, false]);
    }
  });

  it('square the markers, which are the rectangles’ ends', () => {
    expect(markers.get('.note-change-marker')?.get('border-radius')).toBe('0');
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

  it.each(ACCENTS)('fill the marker of a change that was %s', (kind, accent) => {
    const rule = markers.get(`.note-change-marker[data-kind='${kind}']`);
    expect(rule?.get('--marker-color')).toBe(`var(${accent})`);
  });

  it('show a marker at full strength, with nothing round it', () => {
    const marker = markers.get('.note-change-marker');
    expect(marker?.get('background-color')).toBe('var(--marker-color)');
    expect(marker?.has('opacity')).toBe(false);
    // The change being pointed at is shown by its rectangle. A marker has no
    // halo, and no look of its own under a pointer: one the text scrolled
    // under a pointer at rest matches `:hover`, and its change is not lit
    // (noteDiffPointer.ts).
    expect([...markers].filter(([, declarations]) => declarations.has('box-shadow'))).toEqual([]);
    expect([...markers.keys()].filter((selector) => selector.includes(':hover'))).toEqual([]);
    expect([...markers.keys()].filter((selector) => selector.includes('.is-pointed'))).toEqual([]);
  });

  it('line a rectangle with the viewer’s range border at rest', () => {
    const line = blocks.get('.note-diff-block::after');
    expect(line?.get('border-top')).toBe('1px solid var(--block-line, var(--diff-range-border))');
    expect(line?.get('border-bottom')).toBe(
      '1px solid var(--block-line, var(--diff-range-border))'
    );
    // Top and bottom only: the left end is the gutter's, the right the marker's.
    expect(line?.has('border')).toBe(false);
    expect(line?.has('border-left')).toBe(false);
    expect(line?.has('border-right')).toBe(false);
  });

  it.each(ACCENTS)(
    'line the rectangle of a pointed change that was %s in its marker’s colour',
    (kind, accent) => {
      expect(
        blocks.get(`.note-diff-block.is-active[data-kind='${kind}']`)?.get('--block-line')
      ).toBe(`var(${accent})`);
    }
  );

  it.each(ACCENTS.slice(0, 2))(
    'draw the gap line of text that was %s solid when pointed at, and in the tint otherwise',
    (kind, accent, faint) => {
      const rule = blocks.get(`.note-diff-gap[data-kind='${kind}']`);
      expect(rule?.get('--gap-color')).toBe(`var(${accent})`);
      expect(rule?.get('--gap-faint')).toBe(`var(${faint})`);
      expect(blocks.get('.note-diff-gap')?.get('background-color')).toBe('var(--gap-faint)');
      expect(blocks.get('.note-diff-gap.is-active')?.get('background-color')).toBe(
        'var(--gap-color)'
      );
      expect(blocks.get('.note-diff-gap')?.has('opacity')).toBe(false);
    }
  );
});

describe('a note diff’s connectors', () => {
  it('are filled as the rectangles they join, and lined with the viewer’s range border', () => {
    expect(connectors.get('.connector-fill')?.get('fill')).toBe('var(--connector-before)');
    // Two ends of two colours: the band shades from the one to the other.
    expect(connectors.get('.connector-stop-before')?.get('stop-color')).toBe(
      'var(--connector-before)'
    );
    expect(connectors.get('.connector-stop-after')?.get('stop-color')).toBe(
      'var(--connector-after)'
    );
    const stroke = connectors.get('.connector-stroke');
    expect(stroke?.get('stroke')).toBe('var(--connector-stroke, var(--diff-range-border))');
    expect(stroke?.get('stroke-width')).toBe('1px');
    expect(stroke?.get('fill')).toBe('none');
    // The same line the rectangles carry along their top and bottom.
    expect(blocks.get('.note-diff-block::after')?.get('border-top')).toContain(
      'var(--diff-range-border)'
    );
  });

  it.each(KINDS)(
    'take at each end the tint of blocks that were %s, at rest and pointed at',
    (kind, rest, strong) => {
      for (const end of ['before', 'after']) {
        expect(
          connectors.get(`.note-diff-connector[data-${end}='${kind}']`)?.get(`--connector-${end}`)
        ).toBe(`var(${rest})`);
        expect(
          connectors
            .get(`.note-diff-connector.is-pointed[data-${end}='${kind}']`)
            ?.get(`--connector-${end}`)
        ).toBe(`var(${strong})`);
      }
      // The tints the rectangle behind those blocks is filled with.
      const part = blocks.get(`.note-diff-block-part[data-kind='${kind}']`);
      expect([part?.get('--part-tint'), part?.get('--part-strong')]).toEqual([
        `var(${rest})`,
        `var(${strong})`,
      ]);
    }
  );

  it.each([
    ['added', '--status-added'],
    ['removed', '--status-deleted'],
    ['modified', '--diff-modified-accent'],
  ])(
    'line the connector of a pointed change that was %s as its rectangles and its marker',
    (kind, accent) => {
      expect(
        connectors
          .get(`.note-diff-connector.is-pointed[data-kind='${kind}']`)
          ?.get('--connector-stroke')
      ).toBe(`var(${accent})`);
      expect(
        blocks.get(`.note-diff-block.is-active[data-kind='${kind}']`)?.get('--block-line')
      ).toBe(`var(${accent})`);
      expect(markers.get(`.note-change-marker[data-kind='${kind}']`)?.get('--marker-color')).toBe(
        `var(${accent})`
      );
    }
  );

  it('line only the change being pointed at in a colour of its own', () => {
    const lined = [...connectors.keys()].filter((selector) =>
      connectors.get(selector)?.has('--connector-stroke')
    );
    expect(lined).toHaveLength(3);
    expect(lined.every((selector) => selector.includes('.is-pointed'))).toBe(true);
  });

  it('read no token but the viewer’s tints, its range border and the markers’ colours', () => {
    const own = [...tokensRead(styleOf(connectorsSource))].filter(
      (name) => !name.startsWith('--connector-')
    );
    expect(own.sort()).toEqual([
      '--diff-added-bg',
      '--diff-added-bg-emphasis',
      '--diff-modified-accent',
      '--diff-modified-bg',
      '--diff-modified-bg-emphasis',
      '--diff-range-border',
      '--diff-removed-bg',
      '--diff-removed-bg-emphasis',
      '--status-added',
      '--status-deleted',
    ]);
  });
});

describe('the note diff’s stylesheets', () => {
  it('read no colour token in the note’s markup but the viewer’s tints', () => {
    const own = [...tokensRead(noteDiffCss)].filter((name) => !name.startsWith('--note-diff-'));
    expect(own.sort()).toEqual([
      '--diff-added-bg',
      '--diff-added-bg-emphasis',
      '--diff-modified-bg',
      '--diff-modified-bg-emphasis',
      '--diff-modified-inline-bg',
      '--diff-removed-bg',
      '--diff-removed-bg-emphasis',
    ]);
  });

  it('read no colour token behind the changes but the viewer’s tints, its range border and the markers’ colours', () => {
    const own = [...tokensRead(styleOf(blocksSource))].filter(
      (name) => !/^--(?:part|block|gap|note-diff)-/.test(name)
    );
    expect(own.sort()).toEqual([
      '--diff-added-bg',
      '--diff-added-bg-emphasis',
      '--diff-modified-accent',
      '--diff-modified-bg',
      '--diff-modified-bg-emphasis',
      '--diff-range-border',
      '--diff-removed-bg',
      '--diff-removed-bg-emphasis',
      '--status-added',
      '--status-deleted',
    ]);
  });

  it('take the solid colours for lines and markers only, never for a tint', () => {
    expect(noteDiffCss.replace(/\/\*[\s\S]*?\*\//g, '')).not.toMatch(
      /--status-|--diff-modified-accent/
    );
    for (const [selector, declarations] of blocks) {
      for (const [property, value] of declarations) {
        if (!/--status-|--diff-modified-accent/.test(value)) continue;
        expect([selector, ['--block-line', '--gap-color'].includes(property)]).toEqual([
          selector,
          true,
        ]);
      }
    }
  });

  it('never use --status-modified, in any of them', () => {
    for (const css of [
      noteDiffCss,
      styleOf(markersSource),
      styleOf(blocksSource),
      styleOf(connectorsSource),
    ]) {
      expect(tokensRead(css)).not.toContain('--status-modified');
    }
  });

  it.each([
    ['noteDiffStyles.css', noteDiffCss],
    ['NoteDiffMarkers.svelte', styleOf(markersSource)],
    ['NoteDiffBlocks.svelte', styleOf(blocksSource)],
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
      ...[...tokensRead(styleOf(blocksSource))].filter(
        (n) => !/^--(?:part|block|gap|note-diff)-/.test(n)
      ),
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
