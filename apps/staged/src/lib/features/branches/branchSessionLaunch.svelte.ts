import { toast } from 'svelte-sonner';
import type {
  BranchSessionLaunchContext,
  BranchSessionLaunchStatus,
  BranchSessionResponse,
  BranchSessionType,
  BranchTimeline,
} from '../../types';
import * as commands from '../../api/commands';
import { agentState, REMOTE_AGENTS } from '../agents/agent.svelte';
import { getPreferredAgent } from '../settings/preferences.svelte';
import { shouldQueueBranchSession } from './branchSessionQueue';

export type PendingSessionItemType =
  | 'pending-commit'
  | 'generating-note'
  | 'generating-review'
  | 'queued-commit'
  | 'queued-note'
  | 'queued-review';

export type PendingSessionItem = {
  key: string;
  type: PendingSessionItemType;
  title: string;
  secondaryMeta?: string;
  sessionId?: string;
};

const pendingSessionItemsByBranch = $state<Map<string, PendingSessionItem[]>>(new Map());
let pendingSessionItemsVersion = $state(0);

function readPendingSessionItems(branchId: string): PendingSessionItem[] {
  return pendingSessionItemsByBranch.get(branchId) ?? [];
}

function writePendingSessionItems(branchId: string, items: PendingSessionItem[]): void {
  if (items.length > 0) {
    pendingSessionItemsByBranch.set(branchId, items);
  } else {
    pendingSessionItemsByBranch.delete(branchId);
  }
  pendingSessionItemsVersion++;
}

export function getPendingSessionItems(branchId: string): PendingSessionItem[] {
  pendingSessionItemsVersion;
  return readPendingSessionItems(branchId);
}

export function addPendingSession(branchId: string, item: PendingSessionItem): void {
  writePendingSessionItems(branchId, [...readPendingSessionItems(branchId), item]);
}

function sessionModeForPendingType(type: PendingSessionItemType): BranchSessionType {
  switch (type) {
    case 'generating-note':
    case 'queued-note':
      return 'note';
    case 'generating-review':
    case 'queued-review':
      return 'review';
    default:
      return 'commit';
  }
}

function pendingSessionTypeForMode(mode: BranchSessionType): PendingSessionItemType {
  switch (mode) {
    case 'note':
      return 'generating-note';
    case 'review':
      return 'generating-review';
    default:
      return 'pending-commit';
  }
}

function queuedSessionTypeForMode(mode: BranchSessionType): PendingSessionItemType {
  switch (mode) {
    case 'note':
      return 'queued-note';
    case 'review':
      return 'queued-review';
    default:
      return 'queued-commit';
  }
}

function pendingSessionTypeForStatus(
  mode: BranchSessionType,
  status: BranchSessionLaunchStatus
): PendingSessionItemType {
  return status === 'queued' ? queuedSessionTypeForMode(mode) : pendingSessionTypeForMode(mode);
}

export function updatePendingSession(
  branchId: string,
  key: string,
  sessionId: string,
  sessionStatus: BranchSessionLaunchStatus,
  queueMeta?: string
): void {
  writePendingSessionItems(
    branchId,
    readPendingSessionItems(branchId).map((item) => {
      if (item.key !== key) return item;

      const mode = sessionModeForPendingType(item.type);
      return {
        ...item,
        type: pendingSessionTypeForStatus(mode, sessionStatus),
        sessionId,
        secondaryMeta: sessionStatus === 'queued' ? (queueMeta ?? item.secondaryMeta) : undefined,
      };
    })
  );
}

export function removePendingSession(branchId: string, key: string): void {
  writePendingSessionItems(
    branchId,
    readPendingSessionItems(branchId).filter((item) => item.key !== key)
  );
}

export function prunePendingSessionItems(branchId: string, timeline: BranchTimeline): Set<string> {
  const persistedSessionIds = new Set<string>();
  for (const commit of timeline.commits) {
    if (commit.sessionId) persistedSessionIds.add(commit.sessionId);
  }
  for (const note of timeline.notes) {
    if (note.sessionId) persistedSessionIds.add(note.sessionId);
  }
  for (const review of timeline.reviews) {
    if (review.sessionId) persistedSessionIds.add(review.sessionId);
  }

  const prunedSessionIds = new Set<string>();
  const nextItems = readPendingSessionItems(branchId).filter((item) => {
    if (item.sessionId && persistedSessionIds.has(item.sessionId)) {
      prunedSessionIds.add(item.sessionId);
      return false;
    }
    return true;
  });
  writePendingSessionItems(branchId, nextItems);
  return prunedSessionIds;
}

export function hasPendingSessionStart(branchId: string): boolean {
  return getPendingSessionItems(branchId).some((item) => !item.sessionId);
}

export function hasPendingQueuedSession(branchId: string): boolean {
  return getPendingSessionItems(branchId).some(
    (item) =>
      item.type === 'queued-commit' || item.type === 'queued-note' || item.type === 'queued-review'
  );
}

function pendingSessionTitleForMode(mode: BranchSessionType, prompt: string): string {
  const trimmed = prompt.trim();
  if (mode === 'review') return 'Code Review';
  if (trimmed) return trimmed;
  return mode === 'note' ? 'Untitled note' : 'Pending commit';
}

function pendingSessionMetaForMode(mode: BranchSessionType): string {
  switch (mode) {
    case 'note':
      return 'Starting note session...';
    case 'review':
      return 'Starting review session...';
    default:
      return 'Starting commit session...';
  }
}

export function queuedSessionMeta(timeline: BranchTimeline | null | undefined): string {
  return timeline
    ? 'Queued \u2014 waiting for current session\u2026'
    : 'Queued \u2014 waiting for workspace\u2026';
}

interface StartOrQueueBranchSessionOptions {
  branchId: string;
  isRemote: boolean;
  mode: BranchSessionType;
  prompt: string;
  imageIds?: string[];
  launchContext?: BranchSessionLaunchContext;
  getTimeline?: () => BranchTimeline | null;
  onTimelineRefresh?: () => void | Promise<void>;
  willQueueHint?: boolean;
  queueMeta?: string;
  errorTitle?: string;
}

export async function startOrQueueBranchSessionWithPending({
  branchId,
  isRemote,
  mode,
  prompt,
  imageIds = [],
  launchContext,
  getTimeline,
  onTimelineRefresh,
  willQueueHint,
  queueMeta,
  errorTitle = 'Unable to start session',
}: StartOrQueueBranchSessionOptions): Promise<BranchSessionResponse | null> {
  const timeline = getTimeline?.() ?? null;
  const pendingKey = `session-start-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  const pendingQueueMeta = queueMeta ?? queuedSessionMeta(timeline);
  const willQueue =
    willQueueHint ??
    shouldQueueBranchSession({
      mode,
      timeline,
      hasPendingSessionStart: hasPendingSessionStart(branchId),
      hasPendingQueuedSession: hasPendingQueuedSession(branchId),
    });

  addPendingSession(branchId, {
    key: pendingKey,
    type: willQueue ? queuedSessionTypeForMode(mode) : pendingSessionTypeForMode(mode),
    title: pendingSessionTitleForMode(mode, prompt),
    secondaryMeta: willQueue ? pendingQueueMeta : pendingSessionMetaForMode(mode),
  });

  try {
    const agents = isRemote ? REMOTE_AGENTS : agentState.providers;
    const result = await commands.startOrQueueBranchSession(
      branchId,
      prompt,
      mode,
      getPreferredAgent(agents) ?? undefined,
      imageIds.length > 0 ? imageIds : undefined,
      launchContext
    );

    if (!result || !result.sessionId) {
      throw new Error('Failed to start session: no session ID returned');
    }

    updatePendingSession(
      branchId,
      pendingKey,
      result.sessionId,
      result.sessionStatus,
      pendingQueueMeta
    );

    try {
      await onTimelineRefresh?.();
    } catch (e) {
      console.warn('Failed to refresh branch timeline after session start:', e);
    }

    return result;
  } catch (e) {
    removePendingSession(branchId, pendingKey);
    toast.error(errorTitle, {
      description: e instanceof Error ? e.message : String(e),
      duration: Infinity,
    });
    return null;
  }
}
