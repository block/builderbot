/**
 * Pure shaping of the launch-resume offer for the dialog: one row per project
 * (its repo sessions, then its project sessions), which rows start checked,
 * and the session ids a selection sends. Kept free of runes so it is
 * unit-testable without the Svelte plugin.
 */

import type { InterruptedSession, LaunchRecovery } from '../../types';

export interface InterruptedProjectRow {
  /** Stable key for the list and the selection. */
  key: string;
  /** The project's name, or `Unknown project` when it is gone. */
  name: string;
  /** Its interrupted sessions, repo sessions first, in offer order. */
  sessions: InterruptedSession[];
  /** Ids of the sessions checking this row resumes (none of them `manual`). */
  resumableIds: string[];
}

/** How a session reads beside its label in its project's row. */
export function dispositionNote(session: InterruptedSession): string | null {
  switch (session.disposition) {
    case 'resume':
      return null;
    case 'restart':
      return 'will restart from the beginning';
    case 'manual':
      return session.reason ? `not restarted automatically: ${session.reason}` : 'not restarted';
  }
}

function projectKey(session: InterruptedSession): string {
  return `project:${session.projectId ?? session.projectName ?? 'unknown'}`;
}

function allSessions(recovery: LaunchRecovery): InterruptedSession[] {
  return [...recovery.repoSessions, ...recovery.projectSessions];
}

/**
 * One row per project, in the order its first session appears in the offer
 * (repo sessions come first, so projects with branch work lead).
 */
export function groupByProject(recovery: LaunchRecovery): InterruptedProjectRow[] {
  const rows = new Map<string, InterruptedProjectRow>();
  for (const session of allSessions(recovery)) {
    const key = projectKey(session);
    let row = rows.get(key);
    if (!row) {
      row = {
        key,
        name: session.projectName ?? 'Unknown project',
        sessions: [],
        resumableIds: [],
      };
      rows.set(key, row);
    }
    row.sessions.push(session);
    if (session.disposition !== 'manual') row.resumableIds.push(session.sessionId);
  }
  return [...rows.values()];
}

/** A row with nothing that can be resumed is shown, but cannot be checked. */
export function rowSelectable(row: InterruptedProjectRow): boolean {
  return row.resumableIds.length > 0;
}

/** The rows checked when the dialog opens: every one that can be. */
export function defaultSelection(recovery: LaunchRecovery): string[] {
  return groupByProject(recovery)
    .filter(rowSelectable)
    .map((row) => row.key);
}

/**
 * The session ids Resume sends for the checked rows: their resumable sessions
 * in offer order, so repo sessions still go before project sessions. Keys of
 * rows that cannot be resumed (or are not in the offer) send nothing.
 */
export function selectedSessionIds(recovery: LaunchRecovery, selected: Iterable<string>): string[] {
  const keys = new Set(selected);
  return allSessions(recovery)
    .filter((s) => s.disposition !== 'manual' && keys.has(projectKey(s)))
    .map((s) => s.sessionId);
}

/** Every session that can be resumed, whatever is checked. */
export function resumableSessionIds(recovery: LaunchRecovery): string[] {
  return allSessions(recovery)
    .filter((s) => s.disposition !== 'manual')
    .map((s) => s.sessionId);
}

export function interruptedCount(recovery: LaunchRecovery): number {
  return recovery.repoSessions.length + recovery.projectSessions.length;
}

/** The dialog body's first sentence. */
export function interruptedSummary(recovery: LaunchRecovery): string {
  const total = interruptedCount(recovery);
  const resumable = resumableSessionIds(recovery).length;
  const head =
    total === 1
      ? 'Staged was closed while 1 session was running or waiting.'
      : `Staged was closed while ${total} sessions were running or waiting.`;
  if (resumable === total) return head;
  if (resumable === 0) return `${head} None of them can be restarted automatically.`;
  return `${head} ${resumable} of them can be restarted automatically.`;
}
