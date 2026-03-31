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

/**
 * Replace absolute paths that fall within `repoDir` with relative paths.
 * If repoDir is empty/null, returns the text unchanged.
 *
 * When the exact `repoDir` prefix isn't found in the text, ancestor directories
 * are tried (up to 3 levels). This handles the common case where the session's
 * working directory includes a repo subpath (e.g. `worktree_root/apps/staged`)
 * but tool call paths reference the worktree root directly.
 */
export function makePathsRelative(text: string, repoDir: string | null | undefined): string {
  if (!repoDir) return text;

  let dir = repoDir;
  for (let i = 0; i < 4; i++) {
    const prefix = dir.endsWith('/') ? dir : dir + '/';
    if (text.includes(prefix)) {
      return text.replaceAll(prefix, '');
    }
    const parentEnd = dir.lastIndexOf('/');
    if (parentEnd <= 0) break;
    dir = dir.slice(0, parentEnd);
  }

  return text;
}

const TOOL_VERBS: Record<string, [past: string, present: string]> = {
  Run: ['Ran', 'Running'],
  Shell: ['Ran', 'Running'],
  Bash: ['Ran', 'Running'],
  Read: ['Read', 'Reading'],
  ReadFile: ['Read', 'Reading'],
  Write: ['Wrote', 'Writing'],
  WriteFile: ['Wrote', 'Writing'],
  Grep: ['Searched', 'Searching'],
  Search: ['Searched', 'Searching'],
  Glob: ['Listed', 'Listing'],
  Edit: ['Edited', 'Editing'],
  StrReplace: ['Edited', 'Editing'],
  Delete: ['Deleted', 'Deleting'],
  EditNotebook: ['Edited', 'Editing'],
  SemanticSearch: ['Searched', 'Searching'],
};

/** Pick the single most useful display value from structured tool args. */
function primaryArg(toolName: string, args: Record<string, unknown>): string {
  const str = (key: string) => {
    const v = args[key];
    return typeof v === 'string' ? v : undefined;
  };
  switch (toolName) {
    case 'Read':
    case 'ReadFile':
    case 'Write':
    case 'WriteFile':
    case 'Edit':
    case 'Delete':
    case 'EditNotebook':
    case 'StrReplace':
      return str('file_path') || str('path') || '';
    case 'Run':
    case 'Shell':
    case 'Bash':
      return str('command') || str('cmd') || '';
    case 'Grep':
    case 'Search':
    case 'SemanticSearch':
      return str('pattern') || str('query') || '';
    case 'Glob':
      return str('pattern') || str('glob') || '';
    default: {
      const formatted = formatArgs(args);
      return formatted.length > 200 ? formatted.slice(0, 200) + '…' : formatted;
    }
  }
}

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

/** Strip outer markdown code fences (handles truncated content missing the closing fence). */
export function stripCodeFences(content: string): string {
  const trimmed = content.trim();
  const m = trimmed.match(/^```\w*\n([\s\S]*?)(?:\n```\s*$|$)/);
  return m ? m[1].trimEnd() : content;
}

export interface ToolPairDisplay {
  pair: { call: { id: number; content: string }; result: { content: string } | null };
  verb: string;
  detail: string;
}

export interface VerbGroup {
  verb: string;
  items: ToolPairDisplay[];
}

const VERB_NOUNS: Record<string, string> = {
  Read: 'files',
  Reading: 'files',
  Ran: 'commands',
  Running: 'commands',
  Searched: 'searches',
  Searching: 'searches',
  Edited: 'files',
  Editing: 'files',
  Wrote: 'files',
  Writing: 'files',
  Listed: 'listings',
  Listing: 'listings',
  Deleted: 'files',
  Deleting: 'files',
  Explored: 'files',
  Exploring: 'files',
};

export function groupByVerb(
  pairs: { call: { id: number; content: string }; result: { content: string } | null }[],
  repoDir?: string | null,
  forcePastTense?: boolean
): VerbGroup[] {
  const groups: VerbGroup[] = [];
  for (const pair of pairs) {
    const pending = !pair.result && !forcePastTense;
    const { verb, detail } = formatToolDisplay(pair.call.content, repoDir, pending);
    const item: ToolPairDisplay = { pair, verb, detail };
    const last = groups[groups.length - 1];
    if (last && last.verb === verb) {
      last.items.push(item);
    } else {
      groups.push({ verb, items: [item] });
    }
  }
  return groups;
}

export function verbGroupSummary(group: VerbGroup): string {
  const noun = VERB_NOUNS[group.verb] || 'items';
  return `${group.items.length} ${noun}`;
}

export function formatToolDisplay(
  content: string,
  repoDir?: string | null,
  pending?: boolean
): ToolDisplay {
  const tenseIdx = pending ? 1 : 0;
  const parsed = parseToolCall(content);
  if (parsed) {
    // parsed.name may be a bare tool name ("Read") or a full ACP title
    // ("Read /path/to/file"). Try exact match first, then first-word match.
    let toolName = parsed.name;
    let entry = TOOL_VERBS[toolName];
    if (!entry) {
      const spaceIdx = parsed.name.indexOf(' ');
      if (spaceIdx > 0) {
        const firstWord = parsed.name.slice(0, spaceIdx);
        if (TOOL_VERBS[firstWord]) {
          toolName = firstWord;
          entry = TOOL_VERBS[firstWord];
        }
      }
    }
    if (entry) {
      const verb = entry[tenseIdx];
      const detail = makePathsRelative(primaryArg(toolName, parsed.args), repoDir);
      return { verb, detail };
    }
    // Unrecognized tool name — fall through to treat parsed.name as plain text
    content = parsed.name;
  }

  // Plain-text content: check TOOL_VERBS first (handles "Shell", "Bash ls", etc.)
  const spaceIdx = content.indexOf(' ');
  if (spaceIdx > 0) {
    const firstWord = content.slice(0, spaceIdx);
    const tvEntry = TOOL_VERBS[firstWord];
    if (tvEntry) {
      return {
        verb: tvEntry[tenseIdx],
        detail: makePathsRelative(content.slice(spaceIdx + 1), repoDir),
      };
    }
    if (TITLE_VERBS.has(firstWord)) {
      return { verb: firstWord, detail: makePathsRelative(content.slice(spaceIdx + 1), repoDir) };
    }
  } else {
    const tvEntry = TOOL_VERBS[content];
    if (tvEntry) {
      return { verb: tvEntry[tenseIdx], detail: '' };
    }
    if (TITLE_VERBS.has(content)) {
      return { verb: content, detail: '' };
    }
  }

  return { verb: pending ? 'Running' : 'Ran', detail: makePathsRelative(content, repoDir) };
}
