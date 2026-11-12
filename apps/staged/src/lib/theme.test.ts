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

  it('paints inputs white on light chrome and on the lifted surface on dark chrome', () => {
    const light = themeToVarMap(createAdaptiveTheme('#ffffff', '#24292e', '#6e7781'));
    expect(light['--bg-input']).toBe('#ffffff');
    expect(light['--bg-input']).not.toBe(light['--bg-elevated']);

    const dark = themeToVarMap(createAdaptiveTheme('#27212e', '#ffffff', '#91889b'));
    expect(dark['--bg-input']).toBe('#38333f');
    expect(dark['--bg-input']).toBe(dark['--bg-elevated']);
  });
});
