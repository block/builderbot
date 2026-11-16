/**
 * Bringing a diff route in the history up to date before Back or Forward
 * lands on it.
 *
 * A diff route is built by whoever opens the diff (a branch card, a
 * reference) from what they had on screen at that moment: the branch's
 * commits, its labels, the review. While the diff is the view on screen
 * that is all it needs. As a history entry it can be returned to much later,
 * after new commits, a rebase, a rename, or after the branch or the review
 * was deleted. This asks the backend what is true now.
 */

import { getBranch, getBranchTimelineWithRevalidation, getReview } from '../../commands';
import { projectsDataStore } from '../../stores/projectsData.svelte';
import { formatBaseBranch } from '../branches/branchCardHelpers';
import type { DiffDetailRoute } from './navigation.svelte';

/**
 * How long a step waits for the answers. The branch and review are single
 * database reads. The commits come from the timeline the branch cards read,
 * by their rule (getBranchTimelineWithRevalidation): the cached one when it
 * was fetched in the last few seconds, and otherwise a fresh read, which
 * runs git.
 */
export const DIFF_REFRESH_TIMEOUT_MS = 1500;

export type DiffRouteRefresh =
  /** The diff can be shown; `route` is the entry to show in its place. */
  | { status: 'current'; route: DiffDetailRoute }
  /** What the diff is of no longer exists. */
  | { status: 'gone'; what: 'branch' | 'review' }
  /** The backend did not say in time, or failed: nothing is known to be gone. */
  | { status: 'unknown' };

const LATE = Symbol('late');

/**
 * What the route should be now.
 *
 * - `gone` when its branch is deleted, or the review it was opened for is.
 * - `current` otherwise, with the commit list, the branch and base labels
 *   and the project name read from the branch as it is now. The before /
 *   after labels follow when they were the branch's own labels (a commit
 *   diff's `parent` and short sha are left alone).
 * - `unknown` when the branch or review could not be read in time. The
 *   commits alone being late or unreadable is not a reason to hold the
 *   step: the route keeps the list it has.
 *
 * What is being compared (`commitSha`, `scope`), `readonly` and the repo
 * labels are kept as opened. A commit that a rebase has since dropped from
 * the branch still opens as that commit's diff, with the switcher listing
 * the branch's current commits.
 */
export async function refreshDiffRoute(
  route: DiffDetailRoute,
  timeoutMs = DIFF_REFRESH_TIMEOUT_MS
): Promise<DiffRouteRefresh> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<typeof LATE>((resolve) => {
    timer = setTimeout(() => resolve(LATE), timeoutMs);
  });
  try {
    const existence = Promise.all([
      getBranch(route.branchId),
      route.reviewId ? getReview(route.reviewId) : Promise.resolve(undefined),
    ]);
    const { cached, fresh } = getBranchTimelineWithRevalidation(route.branchId);
    const commits = fresh
      ? fresh.then(
          (timeline) => timeline.commits,
          // The branch is gone, or git failed: what was cached, if anything.
          () => cached?.commits
        )
      : Promise.resolve(cached?.commits);
    // Not awaited when the checks below return early.
    existence.catch(() => {});

    const found = await Promise.race([existence, deadline]);
    if (found === LATE) return { status: 'unknown' };
    const [branch, review] = found;
    if (!branch) return { status: 'gone', what: 'branch' };
    if (review === null) return { status: 'gone', what: 'review' };

    const freshCommits = await Promise.race([commits, deadline]);
    const branchLabel = branch.branchName;
    const baseBranchLabel = formatBaseBranch(branch.baseBranch);
    const projectId = route.projectId ?? branch.projectId;
    const project = projectsDataStore.projects.find((candidate) => candidate.id === projectId);
    return {
      status: 'current',
      route: {
        ...route,
        commits: freshCommits === LATE || !freshCommits ? route.commits : freshCommits,
        branchLabel,
        baseBranchLabel,
        beforeLabel:
          route.beforeLabel !== undefined && route.beforeLabel === route.baseBranchLabel
            ? baseBranchLabel
            : route.beforeLabel,
        afterLabel:
          route.afterLabel !== undefined && route.afterLabel === route.branchLabel
            ? branchLabel
            : route.afterLabel,
        projectName: project?.name ?? route.projectName,
      },
    };
  } catch (e) {
    console.warn('[Navigation] Could not check a diff before stepping onto it:', e);
    return { status: 'unknown' };
  } finally {
    clearTimeout(timer);
  }
}
