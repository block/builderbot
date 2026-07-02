import { describe, expect, it } from 'vitest';

import { createAdaptiveTheme, themeToVarMap } from './theme';

describe('createAdaptiveTheme', () => {
  it('exposes a soft themed Pikchr palette for dark chrome', () => {
    const vars = themeToVarMap(
      createAdaptiveTheme('#27212e', '#ffffff', '#91889b', {
        added: '#3fb950',
        deleted: '#f85149',
        modified: '#d29922',
      })
    );

    expect(vars['--diagram-canvas-bg']).toBe('#f2edf8');
    expect(vars['--pikchr-ink']).toBe('#241a2f');
    expect(vars['--pikchr-surface']).toBe('#fffaff');
    expect(vars['--pikchr-muted']).toBe('#74677f');
    expect(vars['--pikchr-red']).toBe('#e97d8f');
    expect(vars['--pikchr-green']).toBe('#69c487');
    expect(vars['--pikchr-blue']).toBe('#77aeea');
    expect(vars['--pikchr-yellow']).toBe('#e9ca6f');
    expect(vars['--pikchr-orange']).toBe('#e7a068');
    expect(vars['--pikchr-yellow']).not.toBe(vars['--pikchr-orange']);
  });

  it('uses a quiet light Pikchr palette for light chrome', () => {
    const vars = themeToVarMap(
      createAdaptiveTheme('#ffffff', '#24292e', '#6e7781', {
        added: '#28a745',
        deleted: '#d73a49',
        modified: '#2188ff',
      })
    );

    expect(vars['--diagram-canvas-bg']).toBe('#fbf8ff');
    expect(vars['--pikchr-surface']).toBe('#ffffff');
    expect(vars['--pikchr-ink']).toBe('#24292e');
    expect(vars['--pikchr-blue']).toBe('#6b9cd7');
    expect(vars['--pikchr-orange']).toBe('#d48855');
    expect(vars['--pikchr-yellow']).not.toBe(vars['--pikchr-orange']);
  });
});
