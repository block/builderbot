import type { UpstreamRelation } from '../../types';

/** Title and badge for a queued pull rendered on a row of its own. */
export type QueuedPullRowCopy = { title: string; meta: string };

/**
 * Copy for a standalone queued-pull footer row, or null when the upstream
 * relation already has a row to host the badge.
 *
 * The "Pull queued" badge and its Cancel button normally ride on the
 * `originAhead` row, since that is the only relation Pull can be started from.
 * But a queued pull outlives that relation: it drains whenever the branch frees
 * up, and a session ahead of it in the queue can land a local commit first,
 * flipping the branch to `diverged` — or to `inSync`/`localAhead`, neither of
 * which renders an upstream row at all. The queued session itself is
 * relation-independent, so without a row of its own it becomes invisible and
 * uncancellable until it drains, which on a diverged branch means a
 * `merge --ff-only` failure the user can no longer call off.
 *
 * `null` relation means the timeline came back without a git state at all (an
 * unprovisioned or removed worktree); the queued session still exists, so it
 * still gets a row.
 */
export function standaloneQueuedPullRowCopy(args: {
  pullQueued: boolean;
  relation: UpstreamRelation | null;
}): QueuedPullRowCopy | null {
  if (!args.pullQueued || args.relation === 'originAhead') return null;
  return { title: 'Pull from origin', meta: 'Pull queued' };
}
