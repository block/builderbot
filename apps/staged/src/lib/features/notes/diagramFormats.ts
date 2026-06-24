export const NOTE_DIAGRAM_FORMATS = [
  {
    language: 'pikchr',
    displayName: 'Pikchr',
    role: 'general',
    recommended: true,
  },
  {
    language: 'mermaid',
    displayName: 'Mermaid',
    role: 'compatibility',
    recommended: false,
  },
  {
    language: 'svg',
    displayName: 'SVG',
    role: 'fallback',
    recommended: false,
  },
] as const;

export type NoteDiagramFormat = (typeof NOTE_DIAGRAM_FORMATS)[number];
export type NoteDiagramLanguage = NoteDiagramFormat['language'];

export interface NoteDiagramFence {
  format: NoteDiagramFormat;
  language: NoteDiagramLanguage;
  infoString: string;
  source: string;
  startLine: number;
  endLine: number | null;
}

const FORMATS_BY_LANGUAGE = new Map<NoteDiagramLanguage, NoteDiagramFormat>(
  NOTE_DIAGRAM_FORMATS.map((format) => [format.language, format])
);

export function normalizeDiagramFenceLanguage(
  infoString: string | null | undefined
): string | null {
  const language = infoString?.trim().split(/\s+/, 1)[0]?.toLowerCase();
  return language || null;
}

export function getNoteDiagramFormat(
  infoString: string | null | undefined
): NoteDiagramFormat | null {
  const language = normalizeDiagramFenceLanguage(infoString);
  if (!language) return null;

  return FORMATS_BY_LANGUAGE.get(language as NoteDiagramLanguage) ?? null;
}

export function isNoteDiagramFormat(infoString: string | null | undefined): boolean {
  return getNoteDiagramFormat(infoString) !== null;
}

export function extractNoteDiagramFences(markdown: string): NoteDiagramFence[] {
  const lines = markdown.split(/\r\n|\n|\r/);
  const diagrams: NoteDiagramFence[] = [];

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
    const format = getNoteDiagramFormat(infoString);
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
