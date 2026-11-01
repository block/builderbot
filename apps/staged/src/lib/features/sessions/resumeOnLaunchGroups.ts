/**
 * Pure shaping of the launch-resume offer for the dialog: repo sessions
 * grouped under their branch, project sessions under their project, with the
 * counts the body sentence and the buttons need. Kept free of runes so it is
 * unit-testable without the Svelte plugin.
 */

import type { InterruptedSession, LaunchRecovery } from '../../types';

export interface InterruptedSessionGroup {
  /** Stable key for the list. */
  key: string;
  /** `Branch fix-login` or `Project Widgets`. */
  heading: string;
  sessions: InterruptedSession[];
}

/** How a row reads beside its label. */
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

function groupBy(
  sessions: InterruptedSession[],
  keyOf: (s: InterruptedSession) => string,
  headingOf: (s: InterruptedSession) => string
): InterruptedSessionGroup[] {
  const groups = new Map<string, InterruptedSessionGroup>();
  for (const session of sessions) {
    const key = keyOf(session);
    let group = groups.get(key);
    if (!group) {
      group = { key, heading: headingOf(session), sessions: [] };
      groups.set(key, group);
    }
    group.sessions.push(session);
  }
  return [...groups.values()];
}

/** Repo sessions under their branch (in offer order), then project sessions under their project. */
export function groupInterruptedSessions(recovery: LaunchRecovery): InterruptedSessionGroup[] {
  const repo = groupBy(
    recovery.repoSessions,
    (s) => `branch:${s.branchId ?? s.branchName ?? 'unknown'}`,
    (s) => {
      const name = s.branchName ?? 'unknown branch';
      return s.projectName ? `${s.projectName} · ${name}` : name;
    }
  );
  const project = groupBy(
    recovery.projectSessions,
    (s) => `project:${s.projectId ?? s.projectName ?? 'unknown'}`,
    (s) => `${s.projectName ?? 'Unknown project'} (project)`
  );
  return [...repo, ...project];
}

/** Every session "Resume all" restarts. */
export function resumableSessionIds(recovery: LaunchRecovery): string[] {
  return [...recovery.repoSessions, ...recovery.projectSessions]
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
