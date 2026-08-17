/**
 * Copy for the quit confirmation dialog.
 *
 * Kept out of the component so the wording is unit-testable: the dialog resolves
 * each session's branch/project names from the stores and passes labels in.
 */

import type { ActiveSessionInfo } from '../../types';

/** How each session type reads in the dialog body. */
const SESSION_TYPE_LABELS: Record<string, string> = {
  note: 'note',
  commit: 'commit',
  review: 'review',
  pr: 'PR',
  push: 'push',
  pull: 'pull',
};

/**
 * Label for one session that a quit would stop, e.g. `review on fix-login` or
 * `commit on fix-login (queued)`.
 *
 * `where` is the branch name when the session belongs to one, otherwise the
 * project name — project-level sessions (notes on a project) have no branch.
 */
export function quitSessionLabel(session: ActiveSessionInfo, where: string | null): string {
  const kind = session.sessionType ? SESSION_TYPE_LABELS[session.sessionType] : null;
  const base = where ? `${kind ?? 'session'} on ${where}` : (kind ?? 'session');
  return session.status === 'queued' ? `${base} (queued)` : base;
}

/**
 * Dialog body: how much stops, what it is, and whether actions go with it.
 *
 * Actions never gate the quit (see `should_prompt` in `app_lifecycle.rs`), so
 * they are mentioned only as a consequence of one.
 */
export function quitPromptDescription(sessionLabels: string[], runningActionCount: number): string {
  const count = sessionLabels.length;
  const sentences = [
    count === 1
      ? '1 session is still running. Quitting will stop it.'
      : `${count} sessions are still running. Quitting will stop them.`,
  ];

  if (sessionLabels.length > 0) {
    sentences.push(`${sessionLabels.join(', ')}.`);
  }

  if (runningActionCount > 0) {
    sentences.push(
      runningActionCount === 1
        ? '1 running action will also stop.'
        : `${runningActionCount} running actions will also stop.`
    );
  }

  return sentences.join(' ');
}
