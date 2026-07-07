import { normalizeDisplayRoots, type DisplayRootInput } from './pathDisplayRoots';
import type { Session } from '../../types';

export interface ParsedToolCall {
  name: string;
  args: Record<string, unknown>;
}

export interface ToolDisplay {
  verb: string;
  detail: string;
}

export function sessionEndMessage(current: Pick<Session, 'completionReason'>): string {
  if (current.completionReason === 'crashed') return 'This session ended unexpectedly.';
  if (current.completionReason === 'app_quit') {
    return 'This session was interrupted when Staged closed.';
  }
  if (current.completionReason === 'project_session_interrupted') {
    return 'This session was stopped by its project session.';
  }
  if (current.completionReason === 'interrupted') {
    return 'You stopped this session.';
  }
  return 'This session can be resumed.';
}

/** Pattern matching XML-tagged context blocks embedded in prompts/messages. */
const XML_BLOCK_PATTERN = /<(action|branch-history|launch-context)>[\s\S]*?<\/\1>/g;

export function hasXmlBlocks(content: string): boolean {
  return /<(action|branch-history|launch-context)>/.test(content);
}

/** Strip XML-tagged context blocks (action, branch-history, launch-context) from display text. */
export function stripXmlTags(text: string): string {
  return text.replace(XML_BLOCK_PATTERN, '').trim();
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
 * Replace absolute paths that fall within any display root with relative paths.
 * If roots are empty/null, returns the text unchanged.
 *
 * Ancestor directories are also tried (up to 3 levels). This handles the
 * common case where the session's working directory includes a repo subpath
 * (e.g. `worktree_root/apps/staged`) but tool call paths reference the
 * worktree root directly.
 */
export function makePathsRelative(text: string, rootsInput: DisplayRootInput): string {
  const roots = normalizeDisplayRoots(rootsInput);
  if (roots.length === 0) return text;

  const direct = replaceRootPrefixes(text, roots);
  const fallback = replaceRootPrefixes(direct.text, ancestorRoots(roots));
  if (direct.matched || fallback.matched) return fallback.text;

  return text;
}

function pathSeparatorFor(root: string): '/' | '\\' {
  return root.includes('\\') && !root.includes('/') ? '\\' : '/';
}

function rootPrefix(root: string): string {
  if (root.endsWith('/') || root.endsWith('\\')) return root;
  return `${root}${pathSeparatorFor(root)}`;
}

function parentRoot(root: string): string | null {
  const trimmed = root.replace(/[/\\]+$/, '');
  const parentEnd = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'));
  if (parentEnd <= 0) return null;

  const parent = trimmed.slice(0, parentEnd);
  if (/^[A-Za-z]:$/.test(parent)) return null;
  return parent;
}

function ancestorRoots(roots: string[]): string[] {
  const ancestors: string[] = [];
  const seen = new Set<string>();
  for (const root of roots) {
    let dir = root;
    for (let i = 0; i < 3; i++) {
      const parent = parentRoot(dir);
      if (!parent) break;
      if (!seen.has(parent)) {
        ancestors.push(parent);
        seen.add(parent);
      }
      dir = parent;
    }
  }
  return ancestors;
}

function replaceRootPrefixes(text: string, roots: string[]): { text: string; matched: boolean } {
  const prefixes = [...new Set(roots.map(rootPrefix))].sort((a, b) => b.length - a.length);
  let result = text;
  let matched = false;

  for (const prefix of prefixes) {
    if (!result.includes(prefix)) continue;
    matched = true;
    result = result.split(prefix).join('');
  }

  return { text: result, matched };
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
  repoDir?: DisplayRootInput,
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

export function verbGroupSummary(group: { verb: string; items: readonly unknown[] }): string {
  const noun = VERB_NOUNS[group.verb] || 'items';
  return `${group.items.length} ${noun}`;
}

export function formatToolDisplay(
  content: string,
  repoDir?: DisplayRootInput,
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
