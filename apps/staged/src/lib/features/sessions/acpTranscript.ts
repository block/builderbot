import type { SessionMessage } from '../../types';
import type { DisplayRootInput } from './pathDisplayRoots';
import { formatToolDisplay, makePathsRelative, stripCodeFences } from './sessionModalHelpers';

export type ToolStatus = 'pending' | 'in_progress' | 'completed' | 'failed' | 'cancelled';

export interface RichToolItem {
  key: string;
  call: SessionMessage;
  result: SessionMessage | null;
  verb: string;
  detail: string;
  status: ToolStatus;
  statusLabel: string;
  statusTone: 'muted' | 'running' | 'success' | 'danger' | 'cancelled';
  toolCallId: string | null;
  toolKind: string | null;
  rawInput: unknown;
  rawOutput: unknown;
  content: unknown;
  locations: unknown;
}

export type AcpTranscriptEventKind =
  | 'plan_update'
  | 'usage_update'
  | 'prompt_response'
  | 'config_options_update'
  | 'session_mode_state'
  | 'current_mode_update'
  | 'available_commands_update'
  | 'session_info_update'
  | 'permission_request';

export interface AcpTranscriptEvent {
  id: number;
  kind: AcpTranscriptEventKind;
  title: string;
  content: unknown;
  message: SessionMessage;
}

export type AcpTranscriptGroup =
  | { type: 'user'; message: SessionMessage }
  | { type: 'assistant'; message: SessionMessage }
  | { type: 'tools'; items: RichToolItem[] }
  | { type: 'acp'; event: AcpTranscriptEvent };

export interface AcpCommand {
  name: string;
  description: string;
  inputHint: string | null;
}

interface ToolAssembly {
  key: string;
  toolCallId: string | null;
  positionId: number;
  call: SessionMessage;
  result: SessionMessage | null;
  metadata: Partial<
    Pick<RichToolItem, 'toolKind' | 'rawInput' | 'rawOutput' | 'content' | 'locations'>
  > & {
    status?: ToolStatus;
  };
}

interface TimelineEntry {
  id: number;
  type: 'visible' | 'hidden-tool' | 'acp-event';
  message?: SessionMessage;
  toolKey?: string;
  event?: AcpTranscriptEvent;
}

const TOOL_EVENT_KINDS = new Set(['tool_call', 'tool_call_update']);
const STANDALONE_EVENT_KINDS = new Set<AcpTranscriptEventKind>([
  'plan_update',
  'usage_update',
  'prompt_response',
  'config_options_update',
  'session_mode_state',
  'current_mode_update',
  'available_commands_update',
  'session_info_update',
  'permission_request',
]);

export function buildAcpTranscriptGroups(
  visibleMessages: SessionMessage[],
  acpMetadataMessages: SessionMessage[],
  displayRoots?: DisplayRootInput
): AcpTranscriptGroup[] {
  const metadataRows = sortedUniqueMessages(
    [...visibleMessages.filter(hasAcpMetadata), ...acpMetadataMessages].filter(hasAcpMetadata)
  );
  const toolAssemblies = assembleTools(visibleMessages, metadataRows);
  const assignedResultIds = new Set<number>();
  for (const item of toolAssemblies.values()) {
    if (item.result) assignedResultIds.add(item.result.id);
  }

  const entries = buildTimelineEntries(
    visibleMessages,
    metadataRows,
    toolAssemblies,
    assignedResultIds
  );
  const groups: AcpTranscriptGroup[] = [];
  const emittedTools = new Set<string>();

  for (const entry of entries) {
    if (entry.type === 'visible') {
      const message = entry.message!;
      if (message.role === 'user') {
        pushNonToolGroup(groups, { type: 'user', message });
      } else if (message.role === 'assistant') {
        pushNonToolGroup(groups, { type: 'assistant', message });
      } else if (message.role === 'tool_call') {
        const key = toolKeyForMessage(message);
        const assembly = toolAssemblies.get(key);
        if (assembly && !emittedTools.has(key)) {
          pushToolGroup(groups, richToolItem(assembly, displayRoots));
          emittedTools.add(key);
        }
      } else if (!assignedResultIds.has(message.id)) {
        const key = `result:${message.id}`;
        const assembly = toolAssemblies.get(key);
        if (assembly && !emittedTools.has(key)) {
          pushToolGroup(groups, richToolItem(assembly, displayRoots));
          emittedTools.add(key);
        }
      }
    } else if (entry.type === 'hidden-tool') {
      const key = entry.toolKey!;
      const assembly = toolAssemblies.get(key);
      if (assembly && !emittedTools.has(key)) {
        pushToolGroup(groups, richToolItem(assembly, displayRoots));
        emittedTools.add(key);
      }
    } else if (entry.event) {
      pushNonToolGroup(groups, { type: 'acp', event: entry.event });
    }
  }

  return groups;
}

export function latestAvailableCommands(metadataMessages: SessionMessage[]): AcpCommand[] {
  const latest = [...metadataMessages]
    .reverse()
    .find((message) => message.acpEventKind === 'available_commands_update');
  const commands = arrayProp(latest?.acpContent, 'availableCommands');
  if (!Array.isArray(commands)) return [];

  return commands
    .map((command) => {
      const name = stringProp(command, 'name');
      if (!name) return null;
      return {
        name,
        description: stringProp(command, 'description') ?? '',
        inputHint: stringProp(objectProp(command, 'input'), 'hint'),
      };
    })
    .filter((command): command is AcpCommand => command !== null);
}

export function formatJson(value: unknown): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

export function textFromAcpContent(content: unknown): string {
  if (!Array.isArray(content)) return '';
  const parts: string[] = [];
  for (const item of content) {
    const type = stringProp(item, 'type');
    if (type === 'content') {
      const block = objectProp(item, 'content');
      if (stringProp(block, 'type') === 'text') {
        const text = stringProp(block, 'text');
        if (text) parts.push(text);
      }
    }
  }
  return parts.join('\n').trim();
}

export interface AcpDiff {
  path: string;
  oldText: string | null;
  newText: string;
}

export function diffsFromAcpContent(content: unknown, displayRoots?: DisplayRootInput): AcpDiff[] {
  if (!Array.isArray(content)) return [];
  const diffs: AcpDiff[] = [];
  for (const item of content) {
    if (stringProp(item, 'type') !== 'diff') continue;
    const path = stringProp(item, 'path');
    const newText = stringProp(item, 'newText');
    if (!path || newText === null) continue;
    diffs.push({
      path: makePathsRelative(path, displayRoots),
      oldText: stringProp(item, 'oldText'),
      newText,
    });
  }
  return diffs;
}

export function terminalRefsFromAcpContent(content: unknown): string[] {
  if (!Array.isArray(content)) return [];
  return content
    .filter((item) => stringProp(item, 'type') === 'terminal')
    .map((item) => stringProp(item, 'terminalId'))
    .filter((id): id is string => !!id);
}

export function simpleUnifiedDiff(diff: AcpDiff): string {
  const oldLines = (diff.oldText ?? '').split('\n');
  const newLines = diff.newText.split('\n');
  if (diff.oldText === null) {
    return newLines.map((line) => `+${line}`).join('\n');
  }
  if (diff.oldText === diff.newText) return diff.newText;

  const output: string[] = [];
  const max = Math.max(oldLines.length, newLines.length);
  for (let i = 0; i < max; i++) {
    const oldLine = oldLines[i];
    const newLine = newLines[i];
    if (oldLine === newLine) {
      if (oldLine !== undefined) output.push(` ${oldLine}`);
    } else {
      if (oldLine !== undefined) output.push(`-${oldLine}`);
      if (newLine !== undefined) output.push(`+${newLine}`);
    }
  }
  return output.join('\n');
}

function sortedUniqueMessages(messages: SessionMessage[]): SessionMessage[] {
  const byId = new Map<number, SessionMessage>();
  for (const message of messages) byId.set(message.id, message);
  return [...byId.values()].sort((a, b) => a.id - b.id);
}

function hasAcpMetadata(message: SessionMessage): boolean {
  return !!message.acpEventKind;
}

function isHiddenMetadata(message: SessionMessage): boolean {
  return message.content === '' && !!message.acpEventKind;
}

function toolKeyForMessage(message: SessionMessage): string {
  return message.acpToolCallId ? `tool:${message.acpToolCallId}` : `row:${message.id}`;
}

function assembleTools(
  visibleMessages: SessionMessage[],
  metadataRows: SessionMessage[]
): Map<string, ToolAssembly> {
  const tools = new Map<string, ToolAssembly>();
  const keyByToolCallId = new Map<string, string>();

  for (const message of visibleMessages) {
    if (message.role !== 'tool_call') continue;
    const key = toolKeyForMessage(message);
    if (message.acpToolCallId) keyByToolCallId.set(message.acpToolCallId, key);
    tools.set(key, {
      key,
      toolCallId: message.acpToolCallId ?? null,
      positionId: message.id,
      call: message,
      result: null,
      metadata: metadataFromMessage(message),
    });
  }

  for (const row of metadataRows) {
    if (!row.acpToolCallId || !TOOL_EVENT_KINDS.has(row.acpEventKind ?? '')) continue;
    const key = keyByToolCallId.get(row.acpToolCallId) ?? `tool:${row.acpToolCallId}`;
    keyByToolCallId.set(row.acpToolCallId, key);
    const existing = tools.get(key);
    if (existing) {
      mergeToolMetadata(existing, row);
      existing.positionId = Math.min(existing.positionId, row.id);
    } else {
      tools.set(key, {
        key,
        toolCallId: row.acpToolCallId,
        positionId: row.id,
        call: row,
        result: null,
        metadata: metadataFromMessage(row),
      });
    }
  }

  assignToolResults(visibleMessages, tools, keyByToolCallId);
  return tools;
}

function assignToolResults(
  visibleMessages: SessionMessage[],
  tools: Map<string, ToolAssembly>,
  keyByToolCallId: Map<string, string>
) {
  let latestUnmatchedToolKey: string | null = null;

  for (const message of visibleMessages) {
    if (message.role === 'tool_call') {
      const key = toolKeyForMessage(message);
      if (!tools.get(key)?.result) latestUnmatchedToolKey = key;
      continue;
    }

    if (message.role !== 'tool_result') {
      if (message.role === 'user') latestUnmatchedToolKey = null;
      continue;
    }

    const keyed = message.acpToolCallId
      ? tools.get(keyByToolCallId.get(message.acpToolCallId) ?? '')
      : null;
    const fallback = latestUnmatchedToolKey ? tools.get(latestUnmatchedToolKey) : null;
    const target = keyed ?? fallback;
    if (target) {
      target.result = message;
      latestUnmatchedToolKey = null;
    } else {
      tools.set(`result:${message.id}`, {
        key: `result:${message.id}`,
        toolCallId: message.acpToolCallId ?? null,
        positionId: message.id,
        call: message,
        result: message,
        metadata: metadataFromMessage(message),
      });
    }
  }
}

function metadataFromMessage(message: SessionMessage): ToolAssembly['metadata'] {
  return {
    status: normalizeToolStatus(message.acpToolStatus),
    toolKind: message.acpToolKind ?? null,
    rawInput: message.acpRawInput,
    rawOutput: message.acpRawOutput,
    content: message.acpContent,
    locations: message.acpLocations,
  };
}

function mergeToolMetadata(tool: ToolAssembly, row: SessionMessage) {
  const next = metadataFromMessage(row);
  if (next.status) tool.metadata.status = next.status;
  if (next.toolKind) tool.metadata.toolKind = next.toolKind;
  if (next.rawInput !== undefined && next.rawInput !== null) tool.metadata.rawInput = next.rawInput;
  if (next.rawOutput !== undefined && next.rawOutput !== null)
    tool.metadata.rawOutput = next.rawOutput;
  if (next.content !== undefined && next.content !== null) tool.metadata.content = next.content;
  if (next.locations !== undefined && next.locations !== null)
    tool.metadata.locations = next.locations;
}

function buildTimelineEntries(
  visibleMessages: SessionMessage[],
  metadataRows: SessionMessage[],
  tools: Map<string, ToolAssembly>,
  assignedResultIds: Set<number>
): TimelineEntry[] {
  const entries: TimelineEntry[] = [];
  const visibleIds = new Set(visibleMessages.map((message) => message.id));
  const visibleToolKeys = new Set(
    visibleMessages
      .filter((message) => message.role === 'tool_call')
      .map((message) => toolKeyForMessage(message))
  );

  for (const message of visibleMessages) {
    if (message.role === 'tool_result' && assignedResultIds.has(message.id)) continue;
    entries.push({ id: message.id, type: 'visible', message });
  }

  for (const tool of tools.values()) {
    if (visibleToolKeys.has(tool.key) || tool.key.startsWith('result:')) continue;
    entries.push({ id: tool.positionId, type: 'hidden-tool', toolKey: tool.key });
  }

  for (const event of standaloneEvents(metadataRows)) {
    if (visibleIds.has(event.id) && !isHiddenMetadata(event.message)) continue;
    entries.push({ id: event.id, type: 'acp-event', event });
  }

  return entries.sort((a, b) => a.id - b.id);
}

function standaloneEvents(metadataRows: SessionMessage[]): AcpTranscriptEvent[] {
  const permissionResponses = new Map<string, unknown>();
  for (const row of metadataRows) {
    if (row.acpEventKind !== 'permission_response') continue;
    const requestId = stringProp(row.acpContent, 'requestId');
    if (requestId) permissionResponses.set(requestId, row.acpContent);
  }

  const events: AcpTranscriptEvent[] = [];
  for (const row of metadataRows) {
    const kind = row.acpEventKind as AcpTranscriptEventKind | undefined;
    if (!kind || !STANDALONE_EVENT_KINDS.has(kind)) continue;
    let content = eventContent(row);
    if (kind === 'permission_request') {
      const requestId = stringProp(row.acpContent, 'requestId');
      content = (requestId && permissionResponses.get(requestId)) || row.acpContent;
    }
    events.push({
      id: row.id,
      kind,
      title: eventTitle(kind),
      content,
      message: row,
    });
  }
  return events;
}

function eventContent(row: SessionMessage): unknown {
  switch (row.acpEventKind) {
    case 'usage_update':
    case 'prompt_response':
      return row.acpUsage ?? row.acpContent;
    case 'config_options_update':
      return row.acpConfigOptions ?? row.acpContent;
    case 'session_mode_state':
      return row.acpSessionModeState ?? row.acpContent;
    case 'session_info_update':
      return row.acpSessionInfo ?? row.acpContent;
    default:
      return row.acpContent;
  }
}

function eventTitle(kind: AcpTranscriptEventKind): string {
  switch (kind) {
    case 'plan_update':
      return 'Plan';
    case 'usage_update':
    case 'prompt_response':
      return 'Usage';
    case 'config_options_update':
      return 'Configuration';
    case 'session_mode_state':
    case 'current_mode_update':
      return 'Mode';
    case 'available_commands_update':
      return 'Slash commands';
    case 'session_info_update':
      return 'Session info';
    case 'permission_request':
      return 'Permission';
  }
}

function richToolItem(tool: ToolAssembly, displayRoots?: DisplayRootInput): RichToolItem {
  const status = tool.metadata.status ?? (tool.result ? 'completed' : 'pending');
  const pending = status === 'pending' || status === 'in_progress';
  const display = formatToolDisplay(tool.call.content, displayRoots, pending);
  return {
    key: tool.key,
    call: tool.call,
    result: tool.result,
    verb: display.verb,
    detail: display.detail,
    status,
    statusLabel: toolStatusLabel(status),
    statusTone: toolStatusTone(status),
    toolCallId: tool.toolCallId,
    toolKind: tool.metadata.toolKind ?? null,
    rawInput: tool.metadata.rawInput,
    rawOutput: tool.metadata.rawOutput,
    content: tool.metadata.content,
    locations: tool.metadata.locations,
  };
}

function normalizeToolStatus(status: string | undefined): ToolStatus | undefined {
  if (!status) return undefined;
  if (status === 'pending') return 'pending';
  if (status === 'in_progress') return 'in_progress';
  if (status === 'completed' || status === 'succeeded' || status === 'success') return 'completed';
  if (status === 'failed' || status === 'error') return 'failed';
  if (status === 'cancelled' || status === 'canceled') return 'cancelled';
  return undefined;
}

function toolStatusLabel(status: ToolStatus): string {
  switch (status) {
    case 'pending':
      return 'Pending';
    case 'in_progress':
      return 'In progress';
    case 'completed':
      return 'Succeeded';
    case 'failed':
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
  }
}

function toolStatusTone(status: ToolStatus): RichToolItem['statusTone'] {
  switch (status) {
    case 'pending':
      return 'muted';
    case 'in_progress':
      return 'running';
    case 'completed':
      return 'success';
    case 'failed':
      return 'danger';
    case 'cancelled':
      return 'cancelled';
  }
}

function pushToolGroup(groups: AcpTranscriptGroup[], item: RichToolItem) {
  const last = groups[groups.length - 1];
  if (last?.type === 'tools') {
    last.items.push(item);
  } else {
    groups.push({ type: 'tools', items: [item] });
  }
}

function pushNonToolGroup(
  groups: AcpTranscriptGroup[],
  group: Exclude<AcpTranscriptGroup, { type: 'tools' }>
) {
  groups.push(group);
}

function objectProp(value: unknown, key: string): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const prop = (value as Record<string, unknown>)[key];
  return prop && typeof prop === 'object' && !Array.isArray(prop)
    ? (prop as Record<string, unknown>)
    : null;
}

function arrayProp(value: unknown, key: string): unknown[] | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const prop = (value as Record<string, unknown>)[key];
  return Array.isArray(prop) ? prop : null;
}

function stringProp(value: unknown, key: string): string | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const prop = (value as Record<string, unknown>)[key];
  return typeof prop === 'string' ? prop : null;
}

export function permissionStatus(content: unknown): string {
  return stringProp(content, 'status') ?? 'pending';
}

export function permissionRequestId(content: unknown): string | null {
  return stringProp(content, 'requestId');
}

export function permissionOptions(
  content: unknown
): { optionId: string; name: string; kind: string }[] {
  const options = (
    content && typeof content === 'object' ? (content as Record<string, unknown>).options : null
  ) as unknown;
  if (!Array.isArray(options)) return [];
  return options
    .map((option) => {
      const optionId = stringProp(option, 'optionId');
      const name = stringProp(option, 'name');
      const kind = stringProp(option, 'kind') ?? '';
      return optionId && name ? { optionId, name, kind } : null;
    })
    .filter(
      (option): option is { optionId: string; name: string; kind: string } => option !== null
    );
}

export function permissionToolTitle(content: unknown, displayRoots?: DisplayRootInput): string {
  return makePathsRelative(stringProp(content, 'toolTitle') ?? '', displayRoots);
}

export function displayLocations(locations: unknown, displayRoots?: DisplayRootInput): string[] {
  if (!Array.isArray(locations)) return [];
  return locations
    .map((location) => {
      const path = stringProp(location, 'path');
      if (!path) return null;
      const line =
        location && typeof location === 'object'
          ? (location as Record<string, unknown>).line
          : undefined;
      const suffix = typeof line === 'number' ? `:${line}` : '';
      return `${makePathsRelative(path, displayRoots)}${suffix}`;
    })
    .filter((location): location is string => !!location);
}

export function toolResultText(item: RichToolItem): string {
  const structuredText = textFromAcpContent(item.content);
  if (structuredText) return structuredText;
  if (typeof item.rawOutput === 'string') return item.rawOutput;
  if (item.rawOutput !== undefined && item.rawOutput !== null) return formatJson(item.rawOutput);
  if (item.result?.content) return stripCodeFences(item.result.content);
  return '';
}
