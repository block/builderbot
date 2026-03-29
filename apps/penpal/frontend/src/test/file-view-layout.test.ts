import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// E-PENPAL-VIEW-MARGINS: verify file view layout CSS enforces symmetric margins
// and no max-width cap on the scroll container.
describe('file view layout margins', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const css = readFileSync(resolve(here, '../index.css'), 'utf-8');

  it('file-main-scroll has symmetric left and right padding', () => {
    // Extract the .file-main-scroll rule
    const match = css.match(/\.file-main-scroll\s*\{([^}]+)\}/);
    expect(match).not.toBeNull();
    const rule = match![1];

    // Padding should include equal left and right values
    const paddingMatch = rule.match(/padding:\s*([^;]+)/);
    expect(paddingMatch).not.toBeNull();
    const padding = paddingMatch![1].trim();

    // Parse the padding shorthand — expect "0 Xpx Ypx Xpx" form where left === right
    const parts = padding.split(/\s+/);
    // 4-value shorthand: top right bottom left
    expect(parts.length).toBe(4);
    const rightPadding = parts[1];
    const leftPadding = parts[3];
    expect(rightPadding).toBe(leftPadding);
  });

  it('file-main-scroll does not have a max-width constraint', () => {
    const match = css.match(/\.file-main-scroll\s*\{([^}]+)\}/);
    expect(match).not.toBeNull();
    const rule = match![1];
    expect(rule).not.toMatch(/max-width/);
  });
});
