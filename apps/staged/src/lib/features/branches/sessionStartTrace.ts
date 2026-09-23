import type { BranchSessionType } from '../../types';

const TRACE_STORAGE_KEY = 'session-start-trace';
const activeTraces = new Map<string, SessionStartTrace>();

export interface SessionStartTrace {
  readonly id: string;
  readonly branchId: string;
  readonly mode: BranchSessionType;
  sessionId?: string;
  readonly startedAt: number;
  lastMarkAt: number;
  endedAt?: number;
}

function isTraceEnabled(): boolean {
  if (!import.meta.env.DEV || typeof window === 'undefined') return false;
  try {
    const value = window.localStorage.getItem(TRACE_STORAGE_KEY);
    return value === '1' || value === 'true' || value === 'on';
  } catch {
    return false;
  }
}

function traceLog(
  event: string,
  trace?: SessionStartTrace | null,
  extra?: Record<string, unknown>
) {
  if (!isTraceEnabled()) return;
  const fields = {
    branchId: trace?.branchId,
    mode: trace?.mode,
    sessionId: trace?.sessionId,
    traceId: trace?.id,
    ...extra,
  };
  console.debug(`[session-start] ${event}`, fields);
}

export function beginSessionStartTrace(
  branchId: string,
  mode: BranchSessionType
): SessionStartTrace | null {
  if (!isTraceEnabled()) return null;
  const trace: SessionStartTrace = {
    id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    branchId,
    mode,
    startedAt: performance.now(),
    lastMarkAt: performance.now(),
  };
  activeTraces.set(branchId, trace);
  traceLog('begin', trace);
  return trace;
}

export function markSessionStartTrace(
  trace: SessionStartTrace | null | undefined,
  event: string,
  extra?: Record<string, unknown>
): void {
  if (!trace || trace.endedAt !== undefined) return;
  const now = performance.now();
  traceLog(event, trace, {
    lapMs: Math.round(now - trace.lastMarkAt),
    totalMs: Math.round(now - trace.startedAt),
    wallClockMs: Date.now(),
    ...extra,
  });
  trace.lastMarkAt = now;
}

export function associateSessionStartTrace(
  trace: SessionStartTrace | null | undefined,
  sessionId: string
): void {
  if (!trace || trace.endedAt !== undefined) return;
  trace.sessionId = sessionId;
  traceLog('session-associated', trace);
}

export function getActiveSessionStartTrace(branchId: string): SessionStartTrace | null {
  const trace = activeTraces.get(branchId);
  return trace && trace.endedAt === undefined ? trace : null;
}

export function markActiveSessionStartTrace(
  branchId: string,
  event: string,
  extra?: Record<string, unknown>
): void {
  markSessionStartTrace(getActiveSessionStartTrace(branchId), event, extra);
}

export function logSessionStartHandler(
  event: 'running handler reload' | 'running handler skip',
  branchId: string,
  sessionId?: string
): void {
  const trace = getActiveSessionStartTrace(branchId);
  if (trace && sessionId && !trace.sessionId) trace.sessionId = sessionId;
  traceLog(event, trace, { eventSessionId: sessionId });
}

function afterFrame(callback: () => void): void {
  if (typeof requestAnimationFrame === 'function') requestAnimationFrame(callback);
  else setTimeout(callback, 0);
}

export function markSessionStartTraceAfterFrame(
  trace: SessionStartTrace | null | undefined,
  event: string,
  extra?: Record<string, unknown>
): void {
  if (!trace) return;
  afterFrame(() => markSessionStartTrace(trace, event, extra));
}

export function markActiveSessionStartTraceAfterFrame(
  branchId: string,
  event: string,
  extra?: Record<string, unknown>
): void {
  const trace = getActiveSessionStartTrace(branchId);
  if (!trace) return;
  markSessionStartTraceAfterFrame(trace, event, extra);
}

export function endSessionStartTrace(
  trace: SessionStartTrace | null | undefined,
  extra?: Record<string, unknown>
): void {
  if (!trace || trace.endedAt !== undefined) return;
  // Let the final DOM update and its row-painted mark happen before closing the trace.
  afterFrame(() => {
    if (trace.endedAt !== undefined) return;
    trace.endedAt = performance.now();
    traceLog('end', trace, {
      durationMs: Math.round(trace.endedAt - trace.startedAt),
      ...extra,
    });
    if (activeTraces.get(trace.branchId) === trace) activeTraces.delete(trace.branchId);
  });
}

export { isTraceEnabled as isSessionStartTraceEnabled };
