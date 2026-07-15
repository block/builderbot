import { describe, expect, it } from 'vitest';

import {
  extractMarkdownDiagramFences,
  fencedDiagramMarkdown,
  getMarkdownDiagramFormat,
  isMarkdownDiagramFormat,
  MARKDOWN_DIAGRAM_FORMATS,
  normalizeDiagramFenceLanguage,
} from './diagramFormats';

describe('markdown diagram format registry', () => {
  it('registers only Pikchr as the recommended general diagram format', () => {
    expect(MARKDOWN_DIAGRAM_FORMATS).toEqual([
      {
        language: 'pikchr',
        displayName: 'Pikchr',
        role: 'general',
        recommended: true,
      },
    ]);
  });

  it('recognizes Pikchr fence languages case-insensitively', () => {
    expect(getMarkdownDiagramFormat('PIKCHR')?.language).toBe('pikchr');
    expect(getMarkdownDiagramFormat('mermaid')).toBeNull();
    expect(getMarkdownDiagramFormat('svg')).toBeNull();
    expect(getMarkdownDiagramFormat('typescript')).toBeNull();
  });

  it('normalizes the first token from a fence info string', () => {
    expect(normalizeDiagramFenceLanguage('  Pikchr title="draft"  ')).toBe('pikchr');
    expect(normalizeDiagramFenceLanguage('')).toBeNull();
  });

  it('reports whether a fence language is a known markdown diagram format', () => {
    expect(isMarkdownDiagramFormat('pikchr')).toBe(true);
    expect(isMarkdownDiagramFormat('rust')).toBe(false);
  });
});

describe('fencedDiagramMarkdown', () => {
  it('wraps diagram source in a pikchr fence', () => {
    expect(fencedDiagramMarkdown('pikchr', 'box "Start" fit')).toBe(
      '```pikchr\nbox "Start" fit\n```'
    );
  });

  it('outgrows backtick runs in the source so they cannot close the fence', () => {
    const source = 'box "label"\n```\ncircle "Hub"';

    const markdown = fencedDiagramMarkdown('pikchr', source);

    expect(markdown).toBe('````pikchr\nbox "label"\n```\ncircle "Hub"\n````');
    expect(extractMarkdownDiagramFences(markdown)).toEqual([
      expect.objectContaining({ language: 'pikchr', source }),
    ]);
  });
});

describe('extractMarkdownDiagramFences', () => {
  it('extracts Pikchr fence metadata and source', () => {
    const diagrams = extractMarkdownDiagramFences(
      [
        '# Note',
        '',
        '```pikchr title="State flow"',
        'box "Start" fit',
        'arrow right 150%',
        'circle "State" fit',
        '```',
      ].join('\n')
    );

    expect(diagrams).toEqual([
      expect.objectContaining({
        language: 'pikchr',
        infoString: 'pikchr title="State flow"',
        source: 'box "Start" fit\narrow right 150%\ncircle "State" fit',
        startLine: 3,
        endLine: 7,
      }),
    ]);
  });

  it('ignores Mermaid and SVG fences as ordinary code fences', () => {
    const diagrams = extractMarkdownDiagramFences(
      ['```mermaid', 'flowchart TD', '```', '~~~svg', '<svg></svg>', '~~~'].join('\n')
    );

    expect(diagrams).toEqual([]);
  });

  it('ignores non-diagram fences while skipping their contents', () => {
    const diagrams = extractMarkdownDiagramFences(
      ['```ts', 'const sample = "```pikchr";', '```', '```pikchr', 'box "Done" fit', '```'].join(
        '\n'
      )
    );

    expect(diagrams).toHaveLength(1);
    expect(diagrams[0].source).toBe('box "Done" fit');
  });

  it('handles an unclosed diagram fence through the end of markdown', () => {
    const diagrams = extractMarkdownDiagramFences(
      ['Before', '```pikchr', 'box "Draft" fit'].join('\n')
    );

    expect(diagrams).toEqual([
      expect.objectContaining({
        language: 'pikchr',
        source: 'box "Draft" fit',
        startLine: 2,
        endLine: null,
      }),
    ]);
  });
});
