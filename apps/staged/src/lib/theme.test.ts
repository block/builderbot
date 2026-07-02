import { describe, expect, it } from 'vitest';

import { createAdaptiveTheme, themeToVarMap } from './theme';

describe('createAdaptiveTheme', () => {
  it('exposes themed Pikchr palette variables for dark chrome', () => {
    const vars = themeToVarMap(
      createAdaptiveTheme('#27212e', '#ffffff', '#91889b', {
        added: '#3fb950',
        deleted: '#f85149',
        modified: '#d29922',
      })
    );

    expect(vars['--diagram-canvas-bg']).not.toBe('#ffffff');
    expect(vars['--pikchr-ink']).toBe('#ffffff');
    expect(vars['--pikchr-surface']).not.toBe('#ffffff');
    expect(vars['--pikchr-muted']).toBe('#91889b');
    expect(vars['--pikchr-red']).toBe('#f85149');
    expect(vars['--pikchr-green']).toBe('#3fb950');
    expect(vars['--pikchr-blue']).toBe('#58a6ff');
    expect(vars['--pikchr-yellow']).toBe('#d29922');
  });

  it('uses a light themed canvas and surface without hard-coding white fills', () => {
    const vars = themeToVarMap(
      createAdaptiveTheme('#ffffff', '#24292e', '#6e7781', {
        added: '#28a745',
        deleted: '#d73a49',
        modified: '#2188ff',
      })
    );

    expect(vars['--diagram-canvas-bg']).not.toBe('#ffffff');
    expect(vars['--pikchr-surface']).not.toBe('#ffffff');
    expect(vars['--pikchr-ink']).toBe('#24292e');
    expect(vars['--pikchr-blue']).toBe('#2188ff');
  });
});
