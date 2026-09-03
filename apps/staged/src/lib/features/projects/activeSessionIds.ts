/**
 * How ProjectSection's active-session-id set responds to a
 * `session-status-changed` event.
 *
 * The set holds the *project* sessions whose live hints are polled and whose
 * children are queried for badges. Two rules, both load-bearing:
 *
 * - Every event returns a **fresh Set**, even when the contents don't change.
 *   Reference identity is what re-runs the badge-refresh `$effect`: a window
 *   that opened mid-flight never saw a child's `running` event, so when that
 *   child finishes the id contents are unchanged and only the reassignment
 *   triggers the refresh that drops its badge.
 *
 * - Repo sessions stay **out** of the set: each would add a
 *   `listChildSessions` round-trip that always comes back empty, since a repo
 *   session's own children never surface as badges. The fresh Set alone is
 *   what makes a parent re-query and pick its new child up.
 *
 * `branchId` is what marks a repo session — they run on a branch, project
 * sessions never do. Not `sessionType`: a resumed project-note session's
 * `running` event carries `sessionType: "note"` too, so keying on the type
 * would evict a resumed parent and freeze its hint and badges.
 */
export function nextActiveSessionIds(
  current: ReadonlySet<string>,
  event: {
    sessionId: string;
    status: string;
    branchId?: string | null;
    sessionType?: string | null;
  }
): Set<string> {
  const next = new Set(current);
  if (event.status === 'running') {
    if (!event.branchId) next.add(event.sessionId);
  } else {
    next.delete(event.sessionId);
  }
  return next;
}
