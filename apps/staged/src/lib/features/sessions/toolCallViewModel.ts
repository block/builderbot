import type { DisplayRootInput } from './pathDisplayRoots';
import type { RichToolItem, ToolStatus } from './acpTranscript';
import { formatJson, terminalRefsFromAcpContent, textFromAcpContent } from './acpTranscript';
import { makePathsRelative, parseToolCall, stripCodeFences } from './sessionModalHelpers';

export type ToolCallCategory = 'edit' | 'command' | 'read' | 'search' | 'network' | 'generic';

export type ToolCallOutputState = 'output' | 'waiting' | 'empty';

export type ToolCallDiffKind = 'created' | 'deleted' | 'modified' | 'unchanged';

export interface ToolCallDiff {
  path: string;
  oldText: string | null;
  newText: string | null;
  kind: ToolCallDiffKind;
}

export interface ToolCallLocation {
  path: string;
  display: string;
  line: number | null;
  column: number | null;
}

export interface ParsedToolCommand {
  type: string | null;
  cmd: string | null;
  path: string | null;
  query: string | null;
  name: string | null;
}

export interface ToolCallMetadata {
  toolKind: string | null;
  toolName: string | null;
  input: Record<string, unknown> | null;
  inputText: string;
  parsedCommands: ParsedToolCommand[];
  command: string | null;
  workingDirectory: string | null;
  targetPath: string | null;
  query: string | null;
  url: string | null;
  method: string | null;
  locations: ToolCallLocation[];
  diffs: ToolCallDiff[];
  terminalRefs: string[];
}

export interface ToolCallOutput {
  state: ToolCallOutputState;
  primaryText: string;
  rawText: string;
  errorText: string;
  stdout: string;
  stderr: string;
  exitCode: number | null;
  emptyLabel: 'Waiting for output' | 'No output' | null;
}

export type ToolCallSection =
  | { kind: 'locations'; locations: ToolCallLocation[] }
  | { kind: 'input'; label: 'Input'; text: string }
  | { kind: 'diff'; diff: ToolCallDiff }
  | { kind: 'terminal_refs'; label: 'Terminal'; refs: string[] }
  | { kind: 'output'; label: string; text: string; tone: 'normal' | 'danger' | 'cancelled' }
  | { kind: 'raw_output'; label: 'Raw output'; text: string }
  | { kind: 'empty'; label: 'Waiting for output' | 'No output' }
  | {
      kind: 'status';
      label: string;
      status: ToolStatus;
      tone: RichToolItem['statusTone'];
    };

export interface ToolCallViewModel {
  key: string;
  category: ToolCallCategory;
  verb: string;
  detail: string;
  status: ToolStatus;
  statusLabel: string;
  statusTone: RichToolItem['statusTone'];
  metadata: ToolCallMetadata;
  output: ToolCallOutput;
  sections: ToolCallSection[];
  hasDetails: boolean;
}

const EDIT_KINDS = new Set([
  'applypatch',
  'apply_patch',
  'delete',
  'edit',
  'editfile',
  'editnotebook',
  'multi_edit',
  'multiedit',
  'patch',
  'replace',
  'strreplace',
  'str_replace',
  'update',
  'write',
  'writefile',
]);

const COMMAND_KINDS = new Set([
  'bash',
  'cmd',
  'command',
  'exec',
  'execute',
  'run',
  'shell',
  'terminal',
]);

const READ_KINDS = new Set(['cat', 'load', 'open', 'read', 'readfile', 'view']);

const SEARCH_KINDS = new Set([
  'find',
  'glob',
  'grep',
  'list',
  'ls',
  'rg',
  'search',
  'semanticsearch',
]);

const NETWORK_KINDS = new Set(['browser', 'curl', 'fetch', 'http', 'network', 'request', 'web']);

const READ_VERBS = new Set(['Read', 'Reading']);
const SEARCH_VERBS = new Set(['Searched', 'Searching', 'Listed', 'Listing']);
const EDIT_VERBS = new Set(['Deleted', 'Deleting', 'Edited', 'Editing', 'Wrote', 'Writing']);
const COMMAND_VERBS = new Set(['Ran', 'Running']);

const PATH_KEYS = ['file_path', 'filePath', 'path', 'filepath', 'filename', 'file'];
const QUERY_KEYS = ['query', 'pattern', 'glob', 'selector'];
const COMMAND_KEYS = ['command', 'cmd', 'script'];
const CWD_KEYS = ['cwd', 'workingDirectory', 'working_directory', 'workdir'];
const URL_KEYS = ['url', 'uri', 'href'];
const METHOD_KEYS = ['method', 'httpMethod', 'http_method'];
const NETWORK_KEYS = new Set([
  'body',
  'headers',
  'method',
  'request',
  'response',
  'screenshot',
  'selector',
  'status',
  'statusCode',
  'status_code',
  'title',
  'url',
]);

export function buildToolCallViewModel(
  item: RichToolItem,
  displayRoots?: DisplayRootInput
): ToolCallViewModel {
  const metadata = extractToolCallMetadata(item, displayRoots);
  const category = classifyToolCall(item, metadata);
  const output = extractToolCallOutput(item);
  const sections = buildToolCallSections(item, metadata, output);

  return {
    key: item.key,
    category,
    verb: item.verb,
    detail: item.detail,
    status: item.status,
    statusLabel: item.statusLabel,
    statusTone: item.statusTone,
    metadata,
    output,
    sections,
    hasDetails: sections.length > 0,
  };
}

export function extractToolCallMetadata(
  item: RichToolItem,
  displayRoots?: DisplayRootInput
): ToolCallMetadata {
  const parsed = parseToolCall(item.call.content);
  const parsedArgs = parsed ? recordOrNull(parsed.args) : null;
  const rawInput = recordOrNull(item.rawInput);
  const input = rawInput ?? parsedArgs;
  const inputText =
    item.rawInput !== undefined && item.rawInput !== null
      ? formatJson(item.rawInput)
      : parsedArgs && Object.keys(parsedArgs).length > 0
        ? formatJson(parsedArgs)
        : '';
  const parsedCommands = [...parsedCommandsFrom(input), ...parsedCommandsFrom(parsedArgs)].filter(
    uniqueParsedCommand
  );
  const diffs = extractToolCallDiffs(item.content, displayRoots);
  const locations = extractToolCallLocations(item.locations, displayRoots);

  return {
    toolKind: normalizeToken(item.toolKind),
    toolName: parsed?.name ?? null,
    input,
    inputText,
    parsedCommands,
    command: relativeValue(
      firstString(input, COMMAND_KEYS) ?? firstCommandValue(parsedCommands),
      displayRoots
    ),
    workingDirectory: relativeValue(firstString(input, CWD_KEYS), displayRoots),
    targetPath: relativeValue(
      firstString(input, PATH_KEYS) ??
        firstParsedCommandField(parsedCommands, 'path') ??
        firstParsedCommandField(parsedCommands, 'name') ??
        diffs[0]?.path ??
        locations[0]?.path ??
        null,
      displayRoots
    ),
    query: firstString(input, QUERY_KEYS) ?? firstParsedCommandField(parsedCommands, 'query'),
    url: firstString(input, URL_KEYS),
    method: normalizeHttpMethod(firstString(input, METHOD_KEYS)),
    locations,
    diffs,
    terminalRefs: terminalRefsFromAcpContent(item.content),
  };
}

export function classifyToolCall(
  item: RichToolItem,
  metadata = extractToolCallMetadata(item)
): ToolCallCategory {
  const toolKindCategory = categoryFromToken(item.toolKind);
  if (toolKindCategory) return toolKindCategory;

  const parsedNameCategory = categoryFromTitle(metadata.toolName);
  if (parsedNameCategory) return parsedNameCategory;

  const parsedCommandCategory = categoryFromParsedCommands(metadata.parsedCommands);
  if (parsedCommandCategory) return parsedCommandCategory;

  const verbCategory = categoryFromVerb(item.verb);
  if (verbCategory) return verbCategory;

  if (metadata.diffs.length > 0) return 'edit';
  if (metadata.terminalRefs.length > 0) return 'command';
  if (hasNetworkMetadata(metadata.input) || hasNetworkMetadata(item.rawOutput)) return 'network';

  return 'generic';
}

export function extractToolCallOutput(item: RichToolItem): ToolCallOutput {
  const rawRecord = recordOrNull(item.rawOutput);
  const rawText = formatJson(item.rawOutput);
  const contentText = textFromAcpContent(item.content);
  const errorText = extractErrorText(rawRecord, item.status);
  const stdout = valueText(firstValue(rawRecord, ['stdout', 'standardOutput', 'standard_output']));
  const stderr = valueText(firstValue(rawRecord, ['stderr', 'standardError', 'standard_error']));
  const exitCode = firstNumber(rawRecord, ['exitCode', 'exit_code', 'code']);
  const structuredOutput = valueText(
    firstValue(rawRecord, ['output', 'text', 'body', 'response', 'content', 'result'])
  );
  const resultText = item.result?.content ? stripCodeFences(item.result.content) : '';
  const primaryText =
    contentText ||
    (typeof item.rawOutput === 'string' ? item.rawOutput : '') ||
    structuredOutput ||
    stdout ||
    stderr ||
    resultText;
  const hasAnyOutput = !!(primaryText || rawText || errorText || stdout || stderr);
  const isWaiting = item.status === 'pending' || item.status === 'in_progress';

  return {
    state: hasAnyOutput ? 'output' : isWaiting ? 'waiting' : 'empty',
    primaryText,
    rawText,
    errorText,
    stdout,
    stderr,
    exitCode,
    emptyLabel: hasAnyOutput ? null : isWaiting ? 'Waiting for output' : 'No output',
  };
}

export function buildToolCallSections(
  item: Pick<RichToolItem, 'status' | 'statusLabel' | 'statusTone'>,
  metadata: ToolCallMetadata,
  output: ToolCallOutput
): ToolCallSection[] {
  const sections: ToolCallSection[] = [];

  if (metadata.locations.length > 0) {
    sections.push({ kind: 'locations', locations: metadata.locations });
  }

  if (metadata.inputText) {
    sections.push({ kind: 'input', label: 'Input', text: metadata.inputText });
  }

  for (const diff of metadata.diffs) {
    sections.push({ kind: 'diff', diff });
  }

  if (metadata.terminalRefs.length > 0) {
    sections.push({ kind: 'terminal_refs', label: 'Terminal', refs: metadata.terminalRefs });
  }

  let hasStructuredOutput = false;
  const outputTone = item.status === 'cancelled' ? 'cancelled' : 'normal';
  if (output.errorText) {
    sections.push({
      kind: 'output',
      label: 'Error',
      text: output.errorText,
      tone: 'danger',
    });
    hasStructuredOutput = true;
  }

  if (output.stdout) {
    sections.push({
      kind: 'output',
      label: 'Stdout',
      text: output.stdout,
      tone: outputTone,
    });
    hasStructuredOutput = true;
  }

  if (output.stderr && output.stderr !== output.errorText) {
    sections.push({
      kind: 'output',
      label: 'Stderr',
      text: output.stderr,
      tone: item.status === 'failed' ? 'danger' : outputTone,
    });
    hasStructuredOutput = true;
  }

  if (
    output.primaryText &&
    output.primaryText !== output.errorText &&
    output.primaryText !== output.stdout &&
    output.primaryText !== output.stderr
  ) {
    sections.push({
      kind: 'output',
      label: 'Output',
      text: output.primaryText,
      tone: outputTone,
    });
    hasStructuredOutput = true;
  }

  // Raw JSON is a fallback for output we could not render structurally,
  // never a companion to structured output sections.
  if (output.rawText && !hasStructuredOutput) {
    sections.push({ kind: 'raw_output', label: 'Raw output', text: output.rawText });
  }

  if (output.emptyLabel) {
    sections.push({ kind: 'empty', label: output.emptyLabel });
  }

  sections.push({
    kind: 'status',
    label: item.statusLabel,
    status: item.status,
    tone: item.statusTone,
  });

  return sections;
}

export function extractToolCallDiffs(
  content: unknown,
  displayRoots?: DisplayRootInput
): ToolCallDiff[] {
  if (!Array.isArray(content)) return [];

  const diffs: ToolCallDiff[] = [];
  for (const block of content) {
    if (stringProp(block, 'type') !== 'diff') continue;
    const path = stringProp(block, 'path');
    if (!path) continue;

    const oldText = nullableStringProp(block, 'oldText');
    const newText = nullableStringProp(block, 'newText');
    if (oldText === null && newText === null) continue;

    diffs.push({
      path: makePathsRelative(path, displayRoots),
      oldText,
      newText,
      kind: diffKind(oldText, newText),
    });
  }

  return diffs;
}

export function extractToolCallLocations(
  locations: unknown,
  displayRoots?: DisplayRootInput
): ToolCallLocation[] {
  if (!Array.isArray(locations)) return [];

  return locations
    .map((location) => {
      const path = stringProp(location, 'path');
      if (!path) return null;
      const displayPath = makePathsRelative(path, displayRoots);
      const line = numberProp(location, 'line');
      const column = numberProp(location, 'column');
      const suffix = `${line !== null ? `:${line}` : ''}${column !== null ? `:${column}` : ''}`;
      return {
        path: displayPath,
        display: `${displayPath}${suffix}`,
        line,
        column,
      };
    })
    .filter((location): location is ToolCallLocation => location !== null);
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function stringProp(value: unknown, key: string): string | null {
  if (!isRecord(value)) return null;
  const prop = value[key];
  return typeof prop === 'string' ? prop : null;
}

function nullableStringProp(value: unknown, key: string): string | null {
  if (!isRecord(value)) return null;
  const prop = value[key];
  return typeof prop === 'string' ? prop : null;
}

function numberProp(value: unknown, key: string): number | null {
  if (!isRecord(value)) return null;
  const prop = value[key];
  return typeof prop === 'number' && Number.isFinite(prop) ? prop : null;
}

function recordOrNull(value: unknown): Record<string, unknown> | null {
  return isRecord(value) ? value : null;
}

function normalizeToken(value: string | null | undefined): string | null {
  if (!value) return null;
  return value.trim().toLowerCase();
}

function compactToken(value: string | null | undefined): string | null {
  const normalized = normalizeToken(value);
  return normalized ? normalized.replace(/[^a-z0-9]/g, '') : null;
}

function categoryFromToken(value: string | null | undefined): ToolCallCategory | null {
  const normalized = normalizeToken(value);
  const compact = compactToken(value);
  if (!normalized || !compact) return null;

  if (EDIT_KINDS.has(normalized) || EDIT_KINDS.has(compact)) return 'edit';
  if (COMMAND_KINDS.has(normalized) || COMMAND_KINDS.has(compact)) return 'command';
  if (READ_KINDS.has(normalized) || READ_KINDS.has(compact)) return 'read';
  if (SEARCH_KINDS.has(normalized) || SEARCH_KINDS.has(compact)) return 'search';
  if (NETWORK_KINDS.has(normalized) || NETWORK_KINDS.has(compact)) return 'network';

  return null;
}

function categoryFromTitle(title: string | null): ToolCallCategory | null {
  if (!title) return null;
  const firstWord = title.trim().split(/\s+/, 1)[0];
  return categoryFromToken(title) ?? categoryFromToken(firstWord);
}

function categoryFromParsedCommands(commands: ParsedToolCommand[]): ToolCallCategory | null {
  for (const command of commands) {
    const typeCategory = categoryFromToken(command.type);
    if (typeCategory) return typeCategory;
    if (command.cmd) return 'command';
  }
  return null;
}

function categoryFromVerb(verb: string): ToolCallCategory | null {
  if (EDIT_VERBS.has(verb)) return 'edit';
  if (COMMAND_VERBS.has(verb)) return 'command';
  if (READ_VERBS.has(verb)) return 'read';
  if (SEARCH_VERBS.has(verb)) return 'search';
  return null;
}

function parsedCommandsFrom(input: Record<string, unknown> | null): ParsedToolCommand[] {
  const parsedCmd = input?.parsed_cmd ?? input?.parsedCmd;
  if (!Array.isArray(parsedCmd)) return [];

  return parsedCmd.filter(isRecord).map((command) => ({
    type: stringProp(command, 'type'),
    cmd: stringProp(command, 'cmd') ?? stringProp(command, 'command'),
    path: stringProp(command, 'path'),
    query: stringProp(command, 'query'),
    name: stringProp(command, 'name'),
  }));
}

function uniqueParsedCommand(
  command: ParsedToolCommand,
  index: number,
  commands: ParsedToolCommand[]
): boolean {
  return (
    commands.findIndex(
      (candidate) =>
        candidate.type === command.type &&
        candidate.cmd === command.cmd &&
        candidate.path === command.path &&
        candidate.query === command.query &&
        candidate.name === command.name
    ) === index
  );
}

function firstString(input: Record<string, unknown> | null, keys: string[]): string | null {
  if (!input) return null;
  for (const key of keys) {
    const value = input[key];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return null;
}

function firstParsedCommandField(
  commands: ParsedToolCommand[],
  field: keyof ParsedToolCommand
): string | null {
  for (const command of commands) {
    const value = command[field];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return null;
}

function firstCommandValue(commands: ParsedToolCommand[]): string | null {
  return firstParsedCommandField(commands, 'cmd');
}

function firstValue(input: Record<string, unknown> | null, keys: string[]): unknown {
  if (!input) return undefined;
  for (const key of keys) {
    if (input[key] !== undefined && input[key] !== null) return input[key];
  }
  return undefined;
}

function firstNumber(input: Record<string, unknown> | null, keys: string[]): number | null {
  if (!input) return null;
  for (const key of keys) {
    const value = input[key];
    if (typeof value === 'number' && Number.isFinite(value)) return value;
  }
  return null;
}

function valueText(value: unknown): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return formatJson(value);
}

function extractErrorText(rawOutput: Record<string, unknown> | null, status: ToolStatus): string {
  const error = firstValue(rawOutput, ['error', 'errorMessage', 'error_message']);
  if (error !== undefined && error !== null) return valueText(error);

  if (status !== 'failed') return '';
  const stderr = valueText(firstValue(rawOutput, ['stderr', 'standardError', 'standard_error']));
  return stderr;
}

function normalizeHttpMethod(method: string | null): string | null {
  return method ? method.toUpperCase() : null;
}

function relativeValue(value: string | null, displayRoots?: DisplayRootInput): string | null {
  return value ? makePathsRelative(value, displayRoots) : null;
}

function hasNetworkMetadata(value: unknown): boolean {
  if (!isRecord(value)) return false;
  return Object.keys(value).some((key) => NETWORK_KEYS.has(key));
}

function diffKind(oldText: string | null, newText: string | null): ToolCallDiffKind {
  if (oldText === null) return 'created';
  if (newText === null) return 'deleted';
  return oldText === newText ? 'unchanged' : 'modified';
}
