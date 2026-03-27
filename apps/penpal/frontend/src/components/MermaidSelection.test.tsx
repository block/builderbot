import { describe, it, expect } from 'vitest';
import { findMermaidFenceLine } from './MermaidSelection';

// E-PENPAL-SVG-STARTLINE: verifies findMermaidFenceLine counts ```mermaid fences to find nth occurrence line.
describe('findMermaidFenceLine', () => {
  it('returns 1-indexed line number for a single mermaid fence (index 0)', () => {
    const md = [
      '# Heading',
      '',
      '```mermaid',
      'graph LR',
      '  A --> B',
      '```',
    ].join('\n');

    expect(findMermaidFenceLine(md, 0)).toBe(3);
  });

  it('returns correct lines for multiple mermaid fences', () => {
    const md = [
      '# Doc',                // line 1
      '',                     // line 2
      '```mermaid',           // line 3
      'graph LR',             // line 4
      '  A --> B',            // line 5
      '```',                  // line 6
      '',                     // line 7
      'Some text.',           // line 8
      '',                     // line 9
      '```mermaid',           // line 10
      'sequenceDiagram',      // line 11
      '  A->>B: Hello',      // line 12
      '```',                  // line 13
      '',                     // line 14
      '```mermaid',           // line 15
      'pie',                  // line 16
      '```',                  // line 17
    ].join('\n');

    expect(findMermaidFenceLine(md, 0)).toBe(3);
    expect(findMermaidFenceLine(md, 1)).toBe(10);
    expect(findMermaidFenceLine(md, 2)).toBe(15);
  });

  it('returns 0 when index is beyond the number of mermaid fences', () => {
    const md = [
      '```mermaid',
      'graph LR',
      '  A --> B',
      '```',
    ].join('\n');

    expect(findMermaidFenceLine(md, 1)).toBe(0);
    expect(findMermaidFenceLine(md, 5)).toBe(0);
  });

  it('returns 0 for empty markdown', () => {
    expect(findMermaidFenceLine('', 0)).toBe(0);
  });

  it('ignores non-mermaid fenced code blocks', () => {
    const md = [
      '```javascript',        // line 1 — not mermaid
      'console.log("hi");',   // line 2
      '```',                  // line 3
      '',                     // line 4
      '```mermaid',           // line 5
      'graph TD',             // line 6
      '```',                  // line 7
    ].join('\n');

    expect(findMermaidFenceLine(md, 0)).toBe(5);
    // There's only one mermaid fence, index 1 should not find anything
    expect(findMermaidFenceLine(md, 1)).toBe(0);
  });

  it('handles mermaid fence with extra attributes after mermaid keyword', () => {
    const md = [
      '```mermaid title="My Diagram"',
      'graph LR',
      '  A --> B',
      '```',
    ].join('\n');

    // The regex /^```mermaid\b/ should still match
    expect(findMermaidFenceLine(md, 0)).toBe(1);
  });

  it('handles indented mermaid fences', () => {
    const md = [
      'Some text',             // line 1
      '',                      // line 2
      '  ```mermaid',          // line 3 — indented, trimmed matches
      '  graph LR',            // line 4
      '  ```',                 // line 5
    ].join('\n');

    expect(findMermaidFenceLine(md, 0)).toBe(3);
  });
});
