import { describe, expect, it } from 'vitest';

import { createAdaptiveTheme, themeToVarMap } from './theme';

describe('createAdaptiveTheme', () => {
  it('exposes a white Pikchr canvas with a soft themed palette for dark chrome', () => {
    const vars = themeToVarMap(
      createAdaptiveTheme('#27212e', '#ffffff', '#91889b', {
        added: '#3fb950',
        deleted: '#f85149',
        modified: '#d29922',
      })
    );

    expect(vars['--diagram-canvas-bg']).toBe('#ffffff');
    expect(vars['--diagram-stage-bg']).toBe('#f2edf8');
    expect(vars['--pikchr-ink']).toBe('#241a2f');
    expect(vars['--pikchr-surface']).toBe('#fffaff');
    expect(vars['--pikchr-muted']).toBe('#74677f');
    expect(vars['--pikchr-red']).toBe('#ec91a0');
    expect(vars['--pikchr-green']).toBe('#80cd99');
    expect(vars['--pikchr-blue']).toBe('#8bbaed');
    expect(vars['--pikchr-yellow']).toBe('#ecd285');
    expect(vars['--pikchr-orange']).toBe('#ebae7f');
    expect(vars['--pikchr-yellow']).not.toBe(vars['--pikchr-orange']);
  });

  it('uses a white Pikchr canvas with a quiet light palette for light chrome', () => {
    const vars = themeToVarMap(
      createAdaptiveTheme('#ffffff', '#24292e', '#6e7781', {
        added: '#28a745',
        deleted: '#d73a49',
        modified: '#2188ff',
      })
    );

    expect(vars['--diagram-canvas-bg']).toBe('#ffffff');
    expect(vars['--diagram-stage-bg']).toBe('#fbf8ff');
    expect(vars['--pikchr-surface']).toBe('#ffffff');
    expect(vars['--pikchr-ink']).toBe('#24292e');
    expect(vars['--pikchr-blue']).toBe('#81abdd');
    expect(vars['--pikchr-orange']).toBe('#da9a6f');
    expect(vars['--pikchr-yellow']).not.toBe(vars['--pikchr-orange']);
  });

  it('gives tags a blue apart from the git palette, which paints "modified" amber in the dark', () => {
    const dark = themeToVarMap(
      createAdaptiveTheme('#27212e', '#ffffff', '#91889b', {
        added: '#3fb950',
        deleted: '#f85149',
        modified: '#d29922',
      })
    );
    // The git blues follow "modified" here, so they are the amber swatch.
    expect(dark['--status-renamed']).toBe('#d29922');
    expect(dark['--status-modified']).toBe(dark['--status-renamed']);
    expect(dark['--tag-blue']).toBe('#58a6ff');

    const light = themeToVarMap(
      createAdaptiveTheme('#ffffff', '#24292e', '#6e7781', {
        added: '#28a745',
        deleted: '#d73a49',
        modified: '#2188ff',
      })
    );
    // Light keeps the blue the tags had before they got their own token.
    expect(light['--tag-blue']).toBe('#2188ff');
    expect(light['--tag-blue']).not.toBe(light['--ui-warning']);
  });

  describe('the emphasised diff tints', () => {
    // The chrome's two themes (DARK_BASE and LIGHT_BASE in preferences).
    const CHROME = {
      dark: createAdaptiveTheme('#27212e', '#ffffff', '#91889b', {
        added: '#3fb950',
        deleted: '#f85149',
        modified: '#d29922',
      }),
      light: createAdaptiveTheme('#ffffff', '#24292e', '#6e7781', {
        added: '#28a745',
        deleted: '#d73a49',
        modified: '#2188ff',
      }),
    };
    // The modified tints are not the theme's: app.css sets one amber for all.
    const MODIFIED_EMPHASIS = 'rgba(227, 179, 65, 0.16)';
    const MODIFIED_INLINE = 'rgba(227, 179, 65, 0.25)';

    const channels = (color: string): number[] =>
      color.startsWith('#')
        ? [1, 3, 5].map((at) => parseInt(color.slice(at, at + 2), 16)).concat(1)
        : (color.match(/[\d.]+/g) ?? []).map(Number);
    /** `top` laid over the opaque `under`. */
    const over = (top: string, under: number[]): number[] => {
      const [r, g, b, a] = channels(top);
      return [r, g, b].map((value, i) => value * a + under[i] * (1 - a));
    };
    const relativeLuminance = (rgb: number[]): number => {
      const [r, g, b] = rgb.map((value) => {
        const v = value / 255;
        return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const contrast = (a: number[], b: number[]): number => {
      const [hi, lo] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x);
      return (hi + 0.05) / (lo + 0.05);
    };

    it.each(['dark', 'light'] as const)(
      'are the %s theme’s own tints, half as strong again or more',
      (mode) => {
        const vars = themeToVarMap(CHROME[mode]);
        for (const kind of ['added', 'removed']) {
          const [r, g, b, a] = channels(vars[`--diff-${kind}-bg`]);
          const [er, eg, eb, ea] = channels(vars[`--diff-${kind}-bg-emphasis`]);
          expect([er, eg, eb]).toEqual([r, g, b]);
          expect(ea / a).toBeGreaterThanOrEqual(1.5);
        }
      }
    );

    it.each(['dark', 'light'] as const)('leave text readable on the %s theme', (mode) => {
      const vars = themeToVarMap(CHROME[mode]);
      const page = channels(vars['--bg-primary']);
      const primary = channels(vars['--text-primary']);
      const secondary = channels(vars['--text-secondary']);
      const blocks = [
        over(vars['--diff-added-bg-emphasis'], page),
        over(vars['--diff-removed-bg-emphasis'], page),
        over(MODIFIED_EMPHASIS, page),
      ];
      for (const block of blocks) {
        expect(contrast(primary, block)).toBeGreaterThanOrEqual(7);
        expect(contrast(secondary, block)).toBeGreaterThanOrEqual(4.5);
      }
      // A changed word, on the strongest tint it is painted over.
      const word = over(MODIFIED_INLINE, over(MODIFIED_EMPHASIS, page));
      expect(contrast(primary, word)).toBeGreaterThanOrEqual(4.5);
    });
  });

  it('paints inputs white on light chrome and on the lifted surface on dark chrome', () => {
    const light = themeToVarMap(createAdaptiveTheme('#ffffff', '#24292e', '#6e7781'));
    expect(light['--bg-input']).toBe('#ffffff');
    expect(light['--bg-input']).not.toBe(light['--bg-elevated']);

    const dark = themeToVarMap(createAdaptiveTheme('#27212e', '#ffffff', '#91889b'));
    expect(dark['--bg-input']).toBe('#38333f');
    expect(dark['--bg-input']).toBe(dark['--bg-elevated']);
  });
});
