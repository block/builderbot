/**
 * Bulk action hydration for repo cards.
 *
 * Every repos surface (the All Repos grid, the home repos row, the sidebar's
 * pinned list) renders one card per repo unvirtualized, and each card needs its
 * configured actions plus any live executions. Asking per card costs two IPC
 * calls each (plus a run-phase call per running execution), and the same repo
 * on two surfaces pays twice.
 *
 * These helpers coalesce that into one call per wave: the first caller defers a
 * microtask, then fires a single bulk command; every caller arriving before it
 * resolves joins the same promise and picks its own slice from the keyed
 * result. Nothing is retained past resolution — no cache, so no invalidation or
 * staleness to reason about. A card hydrating later (cloning flips
 * `hasLocalClone`) simply starts a fresh one-call wave.
 */

import { invokeCommand } from '../../transport';
import { repoActionScopeId, type ProjectAction } from '../../api/commands';
import type { RunningActionSnapshot } from './actions';

/** One context's actions, as returned by `list_all_repo_actions`. */
interface RepoContextActions {
  githubRepo: string;
  subpath: string | null;
  actions: ProjectAction[];
}

/** Key both maps by the scope id, which normalizes an empty subpath away. */
function scopeKey(githubRepo: string, subpath: string | null | undefined): string {
  return repoActionScopeId(githubRepo, subpath ?? undefined);
}

let actionsWave: Promise<Map<string, ProjectAction[]>> | null = null;
let runningWave: Promise<Map<string, RunningActionSnapshot[]>> | null = null;

async function loadActionsWave(): Promise<Map<string, ProjectAction[]>> {
  // Defer one microtask so cards mounting in the same synchronous flush join
  // this wave instead of each starting their own.
  await Promise.resolve();
  try {
    const contexts = await invokeCommand<RepoContextActions[]>('list_all_repo_actions');
    const byScope = new Map<string, ProjectAction[]>();
    for (const context of contexts) {
      byScope.set(scopeKey(context.githubRepo, context.subpath), context.actions);
    }
    return byScope;
  } finally {
    actionsWave = null;
  }
}

async function loadRunningWave(): Promise<Map<string, RunningActionSnapshot[]>> {
  await Promise.resolve();
  try {
    const running = await invokeCommand<RunningActionSnapshot[]>('get_all_running_actions');
    const byScope = new Map<string, RunningActionSnapshot[]>();
    for (const snapshot of running) {
      // branchId carries the opaque scope id: a branch id, or a repo scope id.
      const forScope = byScope.get(snapshot.branchId);
      if (forScope) {
        forScope.push(snapshot);
      } else {
        byScope.set(snapshot.branchId, [snapshot]);
      }
    }
    return byScope;
  } finally {
    runningWave = null;
  }
}

/**
 * The repo context's configured actions. A repo with no context yet resolves to
 * an empty list — the bulk query is read-only, so unlike a per-card
 * `listRepoActions` it never inserts a context row just to render a card.
 */
export function bulkRepoActions(githubRepo: string, subpath?: string): Promise<ProjectAction[]> {
  actionsWave ??= loadActionsWave();
  const key = scopeKey(githubRepo, subpath);
  return actionsWave.then((byScope) => byScope.get(key) ?? []);
}

/** The scope's live executions, each carrying its run phase inline. */
export function bulkRunningForScope(scopeId: string): Promise<RunningActionSnapshot[]> {
  runningWave ??= loadRunningWave();
  return runningWave.then((byScope) => byScope.get(scopeId) ?? []);
}
