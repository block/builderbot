export type ScopeMode = 'brace' | 'indent';

export interface StructuralDeclaration {
  lineIndex: number;
  indent: number;
  kind: 'class' | 'struct' | 'interface' | 'enum' | 'function' | 'method' | 'impl' | string;
  name: string;
  displayText: string;
  endLineIndex?: number;
}

type DeclarationPattern = {
  kind:
    | StructuralDeclaration['kind']
    | ((match: RegExpMatchArray) => StructuralDeclaration['kind']);
  regex: RegExp;
  getName?: (match: RegExpMatchArray) => string;
};

type LanguageConfig = {
  scopeMode: ScopeMode;
  patterns: DeclarationPattern[];
};

const IDENT = String.raw`[A-Za-z_$][\w$]*`;
const BASIC_IDENT = String.raw`[A-Za-z_]\w*`;

const tsLikePatterns: DeclarationPattern[] = [
  {
    kind: (match) => declarationKind(match.groups?.kind ?? 'class'),
    regex: new RegExp(
      String.raw`^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?(?<kind>class|interface|enum)\s+(?<name>${IDENT})\b`
    ),
  },
  {
    kind: 'function',
    regex: new RegExp(
      String.raw`^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+(?<name>${IDENT})\s*\(`
    ),
  },
  {
    kind: 'function',
    regex: new RegExp(
      String.raw`^\s*(?:export\s+)?(?:const|let|var)\s+(?<name>${IDENT})\s*=\s*(?:async\s*)?(?:\([^)]*\)|${IDENT})\s*=>\s*\{`
    ),
  },
  {
    kind: 'method',
    regex: new RegExp(
      String.raw`^\s*(?!(?:if|for|while|switch|catch|function)\b)(?:(?:public|private|protected|static|async|override|abstract|readonly|get|set)\s+)*(?<name>${IDENT}|constructor)\s*(?:<[^>]+>)?\([^)]*\)\s*(?::[^({=>]+)?(?:=>\s*)?\{`
    ),
  },
];

const rustPatterns: DeclarationPattern[] = [
  {
    kind: (match) => declarationKind(match.groups?.kind ?? 'struct'),
    regex: new RegExp(
      String.raw`^\s*(?:pub(?:\([^)]*\))?\s+)?(?<kind>struct|enum|trait)\s+(?<name>${BASIC_IDENT})\b`
    ),
  },
  {
    kind: 'impl',
    regex: /^\s*(?:unsafe\s+)?impl\b(?<name>[^{]*)/,
    getName: (match) => match.groups?.name?.trim() || 'impl',
  },
  {
    kind: 'function',
    regex: new RegExp(
      String.raw`^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]+"\s+)?fn\s+(?<name>${BASIC_IDENT})\b`
    ),
  },
];

const swiftPatterns: DeclarationPattern[] = [
  {
    kind: (match) => declarationKind(match.groups?.kind ?? 'class'),
    regex: new RegExp(
      String.raw`^\s*(?:(?:open|public|private|fileprivate|internal|final|static|class|indirect)\s+)*(?<kind>class|struct|protocol|enum|actor)\s+(?<name>${BASIC_IDENT})\b`
    ),
  },
  {
    kind: 'method',
    regex: new RegExp(
      String.raw`^\s*(?:(?:open|public|private|fileprivate|internal|static|class|override|mutating|nonmutating|final|required|convenience)\s+)*func\s+(?<name>${BASIC_IDENT})\b`
    ),
  },
];

const pythonPatterns: DeclarationPattern[] = [
  {
    kind: 'class',
    regex: new RegExp(String.raw`^\s*class\s+(?<name>${BASIC_IDENT})\b`),
  },
  {
    kind: 'function',
    regex: new RegExp(String.raw`^\s*(?:async\s+)?def\s+(?<name>${BASIC_IDENT})\s*\(`),
  },
];

const goPatterns: DeclarationPattern[] = [
  {
    kind: (match) => declarationKind(match.groups?.kind ?? 'struct'),
    regex: new RegExp(String.raw`^\s*type\s+(?<name>${BASIC_IDENT})\s+(?<kind>struct|interface)\b`),
  },
  {
    kind: (match) => (match.groups?.receiver ? 'method' : 'function'),
    regex: new RegExp(
      String.raw`^\s*func\s+(?<receiver>\([^)]*\)\s*)?(?<name>${BASIC_IDENT})\s*\(`
    ),
  },
];

const javaKotlinPatterns: DeclarationPattern[] = [
  {
    kind: (match) => declarationKind(match.groups?.kind ?? 'class'),
    regex: new RegExp(
      String.raw`^\s*(?:(?:public|private|protected|abstract|final|sealed|open|data|inner|static|value)\s+)*(?<kind>class|interface|enum|object|record)\s+(?<name>${BASIC_IDENT})\b`
    ),
  },
  {
    kind: 'function',
    regex: new RegExp(
      String.raw`^\s*(?:(?:public|private|protected|internal|open|override|suspend|inline|operator|infix|tailrec|final|abstract)\s+)*fun\s+(?:<[^>]+>\s*)?(?<name>${BASIC_IDENT})\s*\(`
    ),
  },
  {
    kind: 'method',
    regex: new RegExp(
      String.raw`^\s*(?!(?:if|for|while|switch|catch|return|new)\b)(?:(?:public|private|protected|static|final|abstract|synchronized|native|default|override)\s+)*(?:[\w<>\[\], ?.@]+\s+)+(?<name>${BASIC_IDENT})\s*\([^;]*\)\s*(?:throws [^{]+)?\{`
    ),
  },
];

const LANGUAGE_CONFIGS: Record<string, LanguageConfig> = {
  '.ts': { scopeMode: 'brace', patterns: tsLikePatterns },
  '.tsx': { scopeMode: 'brace', patterns: tsLikePatterns },
  '.js': { scopeMode: 'brace', patterns: tsLikePatterns },
  '.jsx': { scopeMode: 'brace', patterns: tsLikePatterns },
  '.svelte': { scopeMode: 'brace', patterns: tsLikePatterns },
  '.rs': { scopeMode: 'brace', patterns: rustPatterns },
  '.swift': { scopeMode: 'brace', patterns: swiftPatterns },
  '.py': { scopeMode: 'indent', patterns: pythonPatterns },
  '.go': { scopeMode: 'brace', patterns: goPatterns },
  '.java': { scopeMode: 'brace', patterns: javaKotlinPatterns },
  '.kt': { scopeMode: 'brace', patterns: javaKotlinPatterns },
};

export function getStructuralDeclarations(
  filePath: string | null | undefined,
  lines: string[]
): StructuralDeclaration[] {
  const config = getLanguageConfig(filePath);
  if (!config) return [];

  const declarations = lines
    .map((line, lineIndex) => parseDeclaration(line, lineIndex, config.patterns))
    .filter((declaration): declaration is StructuralDeclaration => declaration !== null);

  return declarations.map((declaration, index) => ({
    ...declaration,
    endLineIndex: getDeclarationEndLine(declaration, lines, config.scopeMode, declarations, index),
  }));
}

export function getActiveStructuralStack(
  declarations: StructuralDeclaration[],
  currentLine: number
): StructuralDeclaration[] {
  const line = Math.max(0, Math.floor(currentLine));

  return declarations
    .filter((declaration) => {
      const endLineIndex = declaration.endLineIndex ?? Number.POSITIVE_INFINITY;
      return declaration.lineIndex <= line && line < endLineIndex;
    })
    .sort((a, b) => a.lineIndex - b.lineIndex || a.indent - b.indent);
}

function getLanguageConfig(filePath: string | null | undefined): LanguageConfig | null {
  if (!filePath) return null;

  const lowerPath = filePath.toLowerCase();
  const extensionIndex = lowerPath.lastIndexOf('.');
  if (extensionIndex === -1) return null;

  const extension = lowerPath.slice(extensionIndex);
  return LANGUAGE_CONFIGS[extension] ?? null;
}

function parseDeclaration(
  line: string,
  lineIndex: number,
  patterns: DeclarationPattern[]
): StructuralDeclaration | null {
  for (const pattern of patterns) {
    const match = line.match(pattern.regex);
    if (!match) continue;

    const name = pattern.getName?.(match) ?? match.groups?.name ?? '';
    if (!name) continue;

    const kind = typeof pattern.kind === 'function' ? pattern.kind(match) : pattern.kind;

    return {
      lineIndex,
      indent: getIndent(line),
      kind,
      name,
      displayText: getDisplayText(line, kind, name),
    };
  }

  return null;
}

function declarationKind(kind: string): StructuralDeclaration['kind'] {
  if (kind === 'protocol') return 'interface';
  if (kind === 'trait') return 'interface';
  if (kind === 'object' || kind === 'record' || kind === 'actor') return 'class';
  return kind;
}

function getIndent(line: string): number {
  let indent = 0;
  for (const char of line) {
    if (char === ' ') {
      indent += 1;
    } else if (char === '\t') {
      indent += 4;
    } else {
      break;
    }
  }
  return indent;
}

function getDisplayText(line: string, kind: StructuralDeclaration['kind'], name: string): string {
  let displayText = removeTrailingLineComment(line).trim();
  const openingBraceIndex = displayText.indexOf('{');
  if (openingBraceIndex !== -1) {
    displayText = displayText.slice(0, openingBraceIndex).trimEnd();
  }
  if (displayText.endsWith(':') || displayText.endsWith(';')) {
    displayText = displayText.slice(0, -1).trimEnd();
  }
  displayText = displayText.replace(/\s+/g, ' ');

  return displayText || `${kind} ${name}`;
}

function removeTrailingLineComment(line: string): string {
  let quote: '"' | "'" | '`' | null = null;

  for (let i = 0; i < line.length; i++) {
    const char = line[i];
    const nextChar = line[i + 1];

    if (quote) {
      if (char === '\\') {
        i++;
        continue;
      }
      if (char === quote) {
        quote = null;
      }
      continue;
    }

    if ((char === '"' || char === "'" || char === '`') && hasClosingQuote(line, i, char)) {
      quote = char;
      continue;
    }

    if (char === '/' && nextChar === '/') {
      return line.slice(0, i);
    }
  }

  return line;
}

function getDeclarationEndLine(
  declaration: StructuralDeclaration,
  lines: string[],
  scopeMode: ScopeMode,
  declarations: StructuralDeclaration[],
  declarationIndex: number
): number {
  if (scopeMode === 'indent') {
    return getIndentScopeEndLine(declaration, lines);
  }

  return getBraceScopeEndLine(declaration, lines, declarations, declarationIndex);
}

function getIndentScopeEndLine(declaration: StructuralDeclaration, lines: string[]): number {
  for (let lineIndex = declaration.lineIndex + 1; lineIndex < lines.length; lineIndex++) {
    const line = lines[lineIndex];
    const trimmed = line.trim();
    if (trimmed === '' || trimmed.startsWith('#')) continue;

    if (getIndent(line) <= declaration.indent) {
      return lineIndex;
    }
  }

  return lines.length;
}

function getBraceScopeEndLine(
  declaration: StructuralDeclaration,
  lines: string[],
  declarations: StructuralDeclaration[],
  declarationIndex: number
): number {
  const nextDeclarationLine = declarations[declarationIndex + 1]?.lineIndex ?? lines.length;
  const openingBrace = findOpeningBrace(lines, declaration.lineIndex, nextDeclarationLine);
  if (!openingBrace) {
    return Math.min(lines.length, declaration.lineIndex + 1);
  }

  const state = { inBlockComment: false };
  let balance = 0;

  for (let lineIndex = declaration.lineIndex; lineIndex < lines.length; lineIndex++) {
    const sanitizedLine = sanitizeBraceLine(lines[lineIndex], state);
    const startColumn = lineIndex === openingBrace.lineIndex ? openingBrace.columnIndex : 0;

    for (let columnIndex = startColumn; columnIndex < sanitizedLine.length; columnIndex++) {
      const char = sanitizedLine[columnIndex];

      if (char === '{') {
        balance += 1;
      } else if (char === '}') {
        balance -= 1;
        if (balance === 0) {
          return lineIndex + 1;
        }
      }
    }
  }

  return lines.length;
}

function findOpeningBrace(
  lines: string[],
  startLineIndex: number,
  stopLineIndex: number
): { lineIndex: number; columnIndex: number } | null {
  const state = { inBlockComment: false };

  for (let lineIndex = startLineIndex; lineIndex < stopLineIndex; lineIndex++) {
    const sanitizedLine = sanitizeBraceLine(lines[lineIndex], state);
    const columnIndex = sanitizedLine.indexOf('{');
    if (columnIndex !== -1) {
      return { lineIndex, columnIndex };
    }
  }

  return null;
}

function sanitizeBraceLine(line: string, state: { inBlockComment: boolean }): string {
  let sanitized = '';
  let quote: '"' | "'" | '`' | null = null;

  for (let i = 0; i < line.length; i++) {
    const char = line[i];
    const nextChar = line[i + 1];

    if (state.inBlockComment) {
      if (char === '*' && nextChar === '/') {
        state.inBlockComment = false;
        sanitized += '  ';
        i++;
      } else {
        sanitized += ' ';
      }
      continue;
    }

    if (quote) {
      if (char === '\\') {
        sanitized += ' ';
        if (nextChar !== undefined) {
          sanitized += ' ';
          i++;
        }
        continue;
      }
      if (char === quote) {
        quote = null;
      }
      sanitized += ' ';
      continue;
    }

    if (char === '/' && nextChar === '*') {
      state.inBlockComment = true;
      sanitized += '  ';
      i++;
      continue;
    }

    if (char === '/' && nextChar === '/') {
      sanitized += ' '.repeat(line.length - i);
      break;
    }

    if ((char === '"' || char === "'" || char === '`') && hasClosingQuote(line, i, char)) {
      quote = char;
      sanitized += ' ';
      continue;
    }

    sanitized += char;
  }

  return sanitized;
}

function hasClosingQuote(line: string, startIndex: number, quote: '"' | "'" | '`'): boolean {
  for (let i = startIndex + 1; i < line.length; i++) {
    const char = line[i];
    if (char === '\\') {
      i++;
      continue;
    }
    if (char === quote) {
      return true;
    }
  }

  return false;
}
