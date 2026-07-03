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
});
