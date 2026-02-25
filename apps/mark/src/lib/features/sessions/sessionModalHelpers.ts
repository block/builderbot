export interface ParsedToolCall {
  name: string;
  args: Record<string, unknown>;
}

export interface ToolDisplay {
  verb: string;
  detail: string;
}

export function hasXmlBlocks(content: string): boolean {
  return /<(action|branch-history)>/.test(content);
}

export function parseToolCall(content: string): ParsedToolCall | null {
  try {
    const parsed = JSON.parse(content);
    if (parsed.name) {
      return {
        name: parsed.name,
        args: parsed.arguments || parsed.args || parsed.input || {},
      };
    }
  } catch {
    // not JSON
  }
  return null;
}

const TOOL_VERBS: Record<string, string> = {
  Shell: 'Ran',
  Bash: 'Ran',
  Read: 'Read',
  ReadFile: 'Read',
  Write: 'Wrote',
  WriteFile: 'Wrote',
  Grep: 'Searched',
  Search: 'Searched',
  Glob: 'Listed',
  StrReplace: 'Edited',
  Delete: 'Deleted',
  EditNotebook: 'Edited',
  SemanticSearch: 'Searched',
};

const TITLE_VERBS = new Set([
  'Add',
  'Analyze',
  'Apply',
  'Browse',
  'Build',
  'Check',
  'Close',
  'Commit',
  'Configure',
  'Connect',
  'Copy',
  'Create',
  'Debug',
  'Delete',
  'Deploy',
  'Download',
  'Edit',
  'Execute',
  'Explore',
  'Fetch',
  'Find',
  'Format',
  'Generate',
  'Get',
  'Initialize',
  'Install',
  'Launch',
  'List',
  'Load',
  'Merge',
  'Monitor',
  'Move',
  'Navigate',
  'Open',
  'Parse',
  'Pull',
  'Push',
  'Ran',
  'Read',
  'Remove',
  'Rename',
  'Reset',
  'Resolve',
  'Restart',
  'Run',
  'Save',
  'Search',
  'Send',
  'Set',
  'Start',
  'Stop',
  'Test',
  'Update',
  'Upload',
  'Validate',
  'Verify',
  'Write',
]);

function formatArgs(args: Record<string, unknown>): string {
  const entries = Object.entries(args);
  if (entries.length === 0) return '';
  return entries
    .map(([key, value]) => {
      const v = typeof value === 'string' ? value : JSON.stringify(value);
      return `${key}: ${v}`;
    })
    .join(', ');
}

export function formatToolDisplay(content: string): ToolDisplay {
  const parsed = parseToolCall(content);
  if (parsed) {
    const verb = TOOL_VERBS[parsed.name] || parsed.name;
    return { verb, detail: formatArgs(parsed.args) };
  }

  const spaceIdx = content.indexOf(' ');
  if (spaceIdx > 0) {
    const firstWord = content.slice(0, spaceIdx);
    if (TITLE_VERBS.has(firstWord)) {
      return { verb: firstWord, detail: content.slice(spaceIdx + 1) };
    }
  } else if (TITLE_VERBS.has(content)) {
    return { verb: content, detail: '' };
  }

  return { verb: 'Ran', detail: content };
}
