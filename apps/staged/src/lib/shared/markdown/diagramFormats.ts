export const MARKDOWN_DIAGRAM_FORMATS = [
  {
    language: 'pikchr',
    displayName: 'Pikchr',
    role: 'general',
    recommended: true,
  },
] as const;

export type MarkdownDiagramFormat = (typeof MARKDOWN_DIAGRAM_FORMATS)[number];
export type MarkdownDiagramLanguage = MarkdownDiagramFormat['language'];

export interface MarkdownDiagramFence {
  format: MarkdownDiagramFormat;
  language: MarkdownDiagramLanguage;
  infoString: string;
  source: string;
  startLine: number;
  endLine: number | null;
}

const FORMATS_BY_LANGUAGE = new Map<MarkdownDiagramLanguage, MarkdownDiagramFormat>(
  MARKDOWN_DIAGRAM_FORMATS.map((format) => [format.language, format])
);

export function normalizeDiagramFenceLanguage(
  infoString: string | null | undefined
): string | null {
  const language = infoString?.trim().split(/\s+/, 1)[0]?.toLowerCase();
  return language || null;
}

export function getMarkdownDiagramFormat(
  infoString: string | null | undefined
): MarkdownDiagramFormat | null {
  const language = normalizeDiagramFenceLanguage(infoString);
  if (!language) return null;

  return FORMATS_BY_LANGUAGE.get(language as MarkdownDiagramLanguage) ?? null;
}

export function isMarkdownDiagramFormat(infoString: string | null | undefined): boolean {
  return getMarkdownDiagramFormat(infoString) !== null;
}

export function extractMarkdownDiagramFences(markdown: string): MarkdownDiagramFence[] {
  const lines = markdown.split(/\r\n|\n|\r/);
  const diagrams: MarkdownDiagramFence[] = [];

  for (let lineIndex = 0; lineIndex < lines.length; lineIndex++) {
    const opening = lines[lineIndex].match(/^ {0,3}(`{3,}|~{3,})([^\n]*)$/);
    if (!opening) continue;

    const openingFence = opening[1];
    const fenceChar = openingFence[0];
    const fenceLength = openingFence.length;
    const infoString = opening[2].trim();
    const contentStart = lineIndex + 1;
    let closingLineIndex: number | null = null;

    for (
      let candidateLineIndex = contentStart;
      candidateLineIndex < lines.length;
      candidateLineIndex++
    ) {
      if (isClosingFence(lines[candidateLineIndex], fenceChar, fenceLength)) {
        closingLineIndex = candidateLineIndex;
        break;
      }
    }

    const contentEnd = closingLineIndex ?? lines.length;
    const format = getMarkdownDiagramFormat(infoString);
    if (format) {
      diagrams.push({
        format,
        language: format.language,
        infoString,
        source: lines.slice(contentStart, contentEnd).join('\n'),
        startLine: lineIndex + 1,
        endLine: closingLineIndex === null ? null : closingLineIndex + 1,
      });
    }

    lineIndex = closingLineIndex ?? lines.length;
  }

  return diagrams;
}

function isClosingFence(line: string, fenceChar: string, fenceLength: number): boolean {
  const escapedFenceChar = fenceChar === '`' ? '`' : '\\~';
  return new RegExp(`^ {0,3}${escapedFenceChar}{${fenceLength},}[ \\t]*$`).test(line);
}
