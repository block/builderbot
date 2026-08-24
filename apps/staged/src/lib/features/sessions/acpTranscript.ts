import type { SessionMessage } from '../../types';
import type { DisplayRootInput } from './pathDisplayRoots';
import { formatToolDisplay, parseToolCall, verbGroupSummary } from './sessionModalHelpers';

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
  isPikchrDiagramTool: boolean;
  innerSessionId: string | null;
  /** Pikchr source of a successful `render_pikchr` call, shown inline as a diagram. */
  pikchrRenderSource: string | null;
}

export interface RichToolVerbGroup {
  key: string;
  verb: string;
  summary: string;
  statusTone: RichToolItem['statusTone'];
  items: RichToolItem[];
}

export type AcpTranscriptEventKind =
  | 'plan_update'
  | 'usage_update'
  | 'prompt_response'
  | 'config_options_update'
  | 'session_mode_state'
  | 'current_mode_update'
  | 'available_commands_update'
  | 'session_info_update';

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
/** Hidden metadata row announcing a `generate_pikchr` child session at start. */
const PIKCHR_SESSION_STARTED_EVENT = 'pikchr_session_started';
const VISIBLE_STANDALONE_EVENT_KINDS = new Set<AcpTranscriptEventKind>();

export function buildAcpTranscriptGroups(
  visibleMessages: SessionMessage[],
  acpMetadataMessages: SessionMessage[],
  displayRoots?: DisplayRootInput
): AcpTranscriptGroup[] {
  const metadataRows = sortedUniqueMessages(
    [...visibleMessages.filter(hasAcpMetadata), ...acpMetadataMessages].filter(hasAcpMetadata)
  );
  const toolAssemblies = assembleTools(visibleMessages, metadataRows);
  const announcedPikchrSessions = assignAnnouncedPikchrSessions(toolAssemblies, metadataRows);
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
          pushToolGroup(groups, richToolItem(assembly, displayRoots, announcedPikchrSessions));
          emittedTools.add(key);
        }
      } else if (!assignedResultIds.has(message.id)) {
        const key = `result:${message.id}`;
        const assembly = toolAssemblies.get(key);
        if (assembly && !emittedTools.has(key)) {
          pushToolGroup(groups, richToolItem(assembly, displayRoots, announcedPikchrSessions));
          emittedTools.add(key);
        }
      }
    } else if (entry.type === 'hidden-tool') {
      const key = entry.toolKey!;
      const assembly = toolAssemblies.get(key);
      if (assembly && !emittedTools.has(key)) {
        pushToolGroup(groups, richToolItem(assembly, displayRoots, announcedPikchrSessions));
        emittedTools.add(key);
      }
    } else if (entry.event) {
      pushNonToolGroup(groups, { type: 'acp', event: entry.event });
    }
  }

  return groups;
}

/** Stable identity key for a transcript group, used both as the keyed-each key
 *  and to pair groups across rebuilds in [`stabilizeAcpTranscriptGroups`]. */
export function transcriptGroupKey(group: AcpTranscriptGroup): string {
  if (group.type === 'tools') return `t-${group.items[0].key}`;
  if (group.type === 'acp') return `a-${group.event.id}`;
  return `m-${group.message.id}`;
}

/**
 * Reuse group (and tool item) object identities from a previous build for
 * entries whose underlying data is unchanged. buildAcpTranscriptGroups creates
 * every object from scratch, which would make a keyed each re-evaluate the
 * whole transcript on every poll tick; with reused identities only groups that
 * actually changed re-render. Returns `previous` itself when nothing changed
 * at all, so downstream deriveds short-circuit too.
 */
export function stabilizeAcpTranscriptGroups(
  previous: AcpTranscriptGroup[],
  next: AcpTranscriptGroup[]
): AcpTranscriptGroup[] {
  if (previous.length === 0) return next;
  const previousByKey = new Map<string, AcpTranscriptGroup>();
  for (const group of previous) previousByKey.set(transcriptGroupKey(group), group);

  const stabilized = next.map((group) => {
    const prior = previousByKey.get(transcriptGroupKey(group));
    return prior ? reuseTranscriptGroup(prior, group) : group;
  });

  if (
    stabilized.length === previous.length &&
    stabilized.every((group, index) => group === previous[index])
  ) {
    return previous;
  }
  return stabilized;
}

function reuseTranscriptGroup(
  prior: AcpTranscriptGroup,
  next: AcpTranscriptGroup
): AcpTranscriptGroup {
  if (prior.type !== next.type) return next;
  if (next.type === 'user' || next.type === 'assistant') {
    return (prior as typeof next).message === next.message ? prior : next;
  }
  if (next.type === 'acp') {
    const priorEvent = (prior as typeof next).event;
    return priorEvent.id === next.event.id &&
      priorEvent.kind === next.event.kind &&
      priorEvent.content === next.event.content &&
      priorEvent.message === next.event.message
      ? prior
      : next;
  }

  const priorItems = (prior as typeof next).items;
  const priorByKey = new Map(priorItems.map((item) => [item.key, item]));
  let reusedInPlace = priorItems.length === next.items.length;
  const items = next.items.map((item, index) => {
    const priorItem = priorByKey.get(item.key);
    if (!priorItem || !richToolItemsEqual(priorItem, item)) {
      reusedInPlace = false;
      return item;
    }
    if (priorItem !== priorItems[index]) reusedInPlace = false;
    return priorItem;
  });
  return reusedInPlace ? prior : { ...next, items };
}

/** All fields are primitives or references into the source message rows, so
 *  strict equality detects change as long as unchanged rows keep identity. */
function richToolItemsEqual(a: RichToolItem, b: RichToolItem): boolean {
  return (
    a.key === b.key &&
    a.call === b.call &&
    a.result === b.result &&
    a.verb === b.verb &&
    a.detail === b.detail &&
    a.status === b.status &&
    a.toolCallId === b.toolCallId &&
    a.toolKind === b.toolKind &&
    a.rawInput === b.rawInput &&
    a.rawOutput === b.rawOutput &&
    a.content === b.content &&
    a.locations === b.locations &&
    a.isPikchrDiagramTool === b.isPikchrDiagramTool &&
    a.innerSessionId === b.innerSessionId &&
    a.pikchrRenderSource === b.pikchrRenderSource
  );
}

/**
 * Whether a metadata row for a tool call has reached a terminal status. Rows
 * that haven't may still be mutated in place by the backend message writer,
 * so incremental metadata polling must re-request them by id.
 */
export function isToolMetadataSettled(message: SessionMessage): boolean {
  const status = normalizeToolStatus(message.acpToolStatus);
  return status === 'completed' || status === 'failed' || status === 'cancelled';
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

export type PlanEntryStatus = 'pending' | 'in_progress' | 'completed' | 'failed';

export interface PlanEntry {
  content: string;
  status: PlanEntryStatus;
}

/**
 * The most recent plan snapshot from a session's ACP metadata rows, or `null`
 * when no plan exists (or the latest plan has no entries). ACP plan updates
 * carry complete snapshots, so latest-row-wins is correct for growth and
 * replacement alike.
 */
export function latestPlan(metadataMessages: SessionMessage[]): PlanEntry[] | null {
  const latest = [...metadataMessages]
    .reverse()
    .find((message) => message.acpEventKind === 'plan_update');
  if (!latest) return null;
  const rawEntries = arrayProp(latest.acpContent, 'entries');
  if (!rawEntries) return null;

  const entries = rawEntries
    .map((entry) => {
      const content = stringProp(entry, 'content');
      if (!content) return null;
      return {
        content,
        status: normalizePlanEntryStatus(stringProp(entry, 'status')),
      };
    })
    .filter((entry): entry is PlanEntry => entry !== null);
  return entries.length > 0 ? entries : null;
}

/** ACP's spec defines pending | in_progress | completed; `failed` is accepted
 *  defensively and anything unknown normalizes to `pending`. */
function normalizePlanEntryStatus(status: string | null): PlanEntryStatus {
  if (status === 'in_progress' || status === 'completed' || status === 'failed') return status;
  return 'pending';
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

export function terminalRefsFromAcpContent(content: unknown): string[] {
  if (!Array.isArray(content)) return [];
  return content
    .filter((item) => stringProp(item, 'type') === 'terminal')
    .map((item) => stringProp(item, 'terminalId'))
    .filter((id): id is string => !!id);
}

/** Tool items that render as their own block (diagram session button, inline
 *  render preview) rather than a generic header row never merge into verb
 *  groups — collapsing them would hide the block. */
function rendersStandalone(item: RichToolItem | undefined): boolean {
  return !!item && (item.isPikchrDiagramTool || item.pikchrRenderSource !== null);
}

export function groupRichToolsByVerb(items: RichToolItem[]): RichToolVerbGroup[] {
  const groups: Array<Omit<RichToolVerbGroup, 'summary' | 'statusTone'>> = [];
  for (const item of items) {
    const last = groups[groups.length - 1];
    if (!rendersStandalone(item) && last?.verb === item.verb && !rendersStandalone(last.items[0])) {
      last.items.push(item);
    } else {
      groups.push({
        key: `verb:${item.key}:${item.verb}`,
        verb: item.verb,
        items: [item],
      });
    }
  }

  return groups.map((group) => ({
    ...group,
    summary: verbGroupSummary(group),
    statusTone: groupStatusTone(group.items),
  }));
}

function groupStatusTone(items: RichToolItem[]): RichToolItem['statusTone'] {
  if (items.some((item) => item.statusTone === 'danger')) return 'danger';
  if (items.some((item) => item.statusTone === 'running')) return 'running';
  if (items.some((item) => item.statusTone === 'cancelled')) return 'cancelled';
  if (items.length > 0 && items.every((item) => item.statusTone === 'success')) return 'success';
  return 'muted';
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
  const events: AcpTranscriptEvent[] = [];
  for (const row of metadataRows) {
    const kind = row.acpEventKind as AcpTranscriptEventKind | undefined;
    if (!kind || !VISIBLE_STANDALONE_EVENT_KINDS.has(kind)) continue;
    events.push({
      id: row.id,
      kind,
      title: eventTitle(kind),
      content: eventContent(row),
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
      return 'Plan'; // Unreachable in transcript rows; kept for kind exhaustiveness.
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
  }
}

function richToolItem(
  tool: ToolAssembly,
  displayRoots?: DisplayRootInput,
  announcedPikchrSessions?: Map<string, string>
): RichToolItem {
  const status = tool.metadata.status ?? (tool.result ? 'completed' : 'pending');
  const pending = status === 'pending' || status === 'in_progress';
  const display = formatToolDisplay(tool.call.content, displayRoots, pending);
  const isPikchrDiagramTool = isPikchrTool(tool);
  // A successful tool result names the child session authoritatively; tools
  // without an output-derived id — still running, or failed before the result
  // could carry one (e.g. a timeout) — fall back to the start-of-run
  // announcement, so the diagram session stays reachable mid-generation and
  // after a failure (its transcript and status record what went wrong).
  const announcedSessionId = announcedPikchrSessions?.get(tool.key) ?? null;
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
    isPikchrDiagramTool,
    innerSessionId: isPikchrDiagramTool
      ? (extractInnerSessionId(tool) ?? announcedSessionId)
      : null,
    pikchrRenderSource: pikchrRenderSourceForTool(tool, status),
  };
}

function isPikchrTool(tool: ToolAssembly): boolean {
  return [tool.call.content, tool.metadata.toolKind]
    .map(normalizedToolName)
    .some((name) => /(?:^|[._]+)generate_pikchr$/.test(name));
}

/**
 * The Pikchr source a successful `render_pikchr` call previewed — the tool the
 * diagram specialist iterates with inside a `generate_pikchr` child session.
 * Only completed calls surface their source: those passed the render gate, so
 * the transcript shows them inline as diagrams; failed or still-running calls
 * keep the regular tool card with their error details.
 */
function pikchrRenderSourceForTool(tool: ToolAssembly, status: ToolStatus): string | null {
  if (status !== 'completed' || !isPikchrRenderTool(tool)) return null;
  return (
    pikchrSourceFromInput(tool.metadata.rawInput) ??
    pikchrSourceFromInput(parseToolCall(tool.call.content)?.args)
  );
}

function isPikchrRenderTool(tool: ToolAssembly): boolean {
  return [tool.call.content, tool.metadata.toolKind]
    .map(normalizedToolName)
    .some((name) => /(?:^|[._]+)render_pikchr$/.test(name));
}

function pikchrSourceFromInput(input: unknown): string | null {
  if (!input || typeof input !== 'object' || Array.isArray(input)) return null;
  const source = (input as Record<string, unknown>).pikchr;
  return typeof source === 'string' && source.trim() ? source : null;
}

function normalizedToolName(value: unknown): string {
  if (typeof value !== 'string') return '';
  const parsed = parseToolCall(value);
  const name = (parsed?.name ?? value).trim().split(/\s+/)[0] ?? '';
  return name
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, '_');
}

/**
 * Pair `pikchr_session_started` announcements with the Pikchr tool items they
 * belong to, keyed by tool key. The backend writes an announcement into the
 * parent transcript the moment `generate_pikchr` creates its child session —
 * before any tool output exists — so the transcript can link to the diagram
 * session mid-run. Ids already claimed by a tool's output are skipped; each
 * remaining announcement pairs with the nearest preceding Pikchr tool lacking
 * an output-derived id — the backend records the tool_call row before the
 * announcement, so that tool is the announcement's own call. Older unmatched
 * tools (legacy transcripts, failed announcement writes) stay unclaimed
 * rather than stealing a later call's link.
 */
function assignAnnouncedPikchrSessions(
  tools: Map<string, ToolAssembly>,
  metadataRows: SessionMessage[]
): Map<string, string> {
  const assigned = new Map<string, string>();
  const announcements = metadataRows.filter(
    (row) => row.acpEventKind === PIKCHR_SESSION_STARTED_EVENT
  );
  if (announcements.length === 0) return assigned;

  const claimed = new Set<string>();
  const unmatched: ToolAssembly[] = [];
  for (const tool of [...tools.values()].sort((a, b) => a.positionId - b.positionId)) {
    if (!isPikchrTool(tool)) continue;
    const fromOutput = extractInnerSessionId(tool);
    if (fromOutput) claimed.add(fromOutput);
    else unmatched.push(tool);
  }

  for (const row of announcements) {
    const sessionId = innerSessionIdFromValue(row.acpContent);
    if (!sessionId || claimed.has(sessionId)) continue;
    // `unmatched` is sorted ascending, so the last entry before the row is
    // the nearest preceding tool.
    let index = -1;
    for (let i = unmatched.length - 1; i >= 0; i--) {
      if (unmatched[i].positionId < row.id) {
        index = i;
        break;
      }
    }
    // An announcement racing ahead of its tool_call row still pairs with the
    // earliest unmatched tool rather than being dropped.
    const tool = index === -1 ? unmatched.shift() : unmatched.splice(index, 1)[0];
    if (!tool) continue;
    assigned.set(tool.key, sessionId);
  }
  return assigned;
}

function extractInnerSessionId(tool: ToolAssembly): string | null {
  return (
    innerSessionIdFromValue(tool.metadata.rawOutput) ??
    innerSessionIdFromValue(tool.metadata.content) ??
    innerSessionIdFromValue(tool.result?.acpRawOutput) ??
    innerSessionIdFromValue(tool.result?.acpContent) ??
    innerSessionIdFromValue(tool.result?.content) ??
    null
  );
}

function innerSessionIdFromValue(value: unknown): string | null {
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (!trimmed.startsWith('{') && !trimmed.startsWith('[')) return null;
    try {
      return innerSessionIdFromValue(JSON.parse(trimmed));
    } catch {
      return null;
    }
  }
  if (!value || typeof value !== 'object') return null;

  if (Array.isArray(value)) {
    for (const item of value) {
      const sessionId = innerSessionIdFromValue(item);
      if (sessionId) return sessionId;
    }
    return null;
  }

  const record = value as Record<string, unknown>;
  const direct = stringValue(record.innerSessionId) ?? stringValue(record.inner_session_id);
  if (direct) return direct;

  for (const key of [
    'structuredContent',
    'structured_content',
    'meta',
    'metadata',
    'data',
    'result',
  ]) {
    const sessionId = innerSessionIdFromValue(record[key]);
    if (sessionId) return sessionId;
  }

  return null;
}

function stringValue(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
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
