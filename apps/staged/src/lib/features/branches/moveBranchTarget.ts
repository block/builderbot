/**
 * Which projects a branch can move to, and why a chosen one can't take it.
 *
 * MoveBranchDialog's list and its disabled Move button both read from here, so
 * the rules the backend enforces have exactly one frontend statement.
 */
import { matchesRepoSearch } from '../../shared/repoSearch';
import { projectDisplayName } from '../../shared/utils';
import type { Project, ProjectRepo } from '../../types';

/** The repo a branch travels with: `githubRepo` plus its optional subpath. */
export interface BranchRepo {
  githubRepo: string;
  subpath: string | null;
}

/**
 * A repo's identity inside a project, keyed the way the backend's
 * `idx_project_repos_unique` does — `(github_repo, COALESCE(subpath, ''))`, so a
 * NULL subpath and an empty one are the same repo.
 */
export function repoKey(githubRepo: string, subpath: string | null | undefined): string {
  return `${githubRepo}\x00${subpath ?? ''}`;
}

/**
 * The repo the branch moves with: its own `project_repos` row when it has one,
 * else its project's denormalized primary — mirroring the backend's
 * `resolve_branch_repo_slug` fallback.
 */
export function branchRepoIdentity(
  repo: ProjectRepo | null | undefined,
  sourceProject: Project | null | undefined
): BranchRepo | null {
  if (repo) {
    return { githubRepo: repo.githubRepo, subpath: repo.subpath };
  }
  if (sourceProject?.githubRepo) {
    return { githubRepo: sourceProject.githubRepo, subpath: sourceProject.subpath };
  }
  return null;
}

export function describeBranchRepo(repo: BranchRepo): string {
  return repo.subpath ? `${repo.githubRepo} (${repo.subpath})` : repo.githubRepo;
}

/** Match projects on their name or on any attached repo path. */
export function filterMoveTargets(
  candidates: Project[],
  reposByProject: Map<string, ProjectRepo[]>,
  query: string
): Project[] {
  const trimmed = query.trim();
  if (!trimmed) return candidates;
  const lower = trimmed.toLowerCase();
  return candidates.filter((project) => {
    if (projectDisplayName(project).toLowerCase().includes(lower)) return true;
    return (reposByProject.get(project.id) ?? []).some((repo) =>
      matchesRepoSearch(repo.githubRepo, repo.subpath, trimmed)
    );
  });
}

/**
 * Why `target` can't receive the branch, or `null` when it can.
 *
 * `targetRepos` is `undefined` for a project whose repos haven't been fetched:
 * the duplicate check would be a guess, so this returns `null` and callers gate
 * on [`isMoveTargetChecking`] instead of letting the move through.
 */
export function moveTargetInvalidReason(
  target: Project,
  branchRepo: BranchRepo | null,
  targetRepos: ProjectRepo[] | undefined
): string | null {
  // Remote projects share one Blox workspace across their branches, so a move
  // in would mean cross-workspace surgery on a filesystem we don't own.
  if (target.location === 'remote') return "Remote projects can't receive branches.";
  if (!targetRepos || !branchRepo) return null;
  const key = repoKey(branchRepo.githubRepo, branchRepo.subpath);
  if (targetRepos.some((repo) => repoKey(repo.githubRepo, repo.subpath) === key)) {
    return `${projectDisplayName(target)} already has ${describeBranchRepo(branchRepo)} attached.`;
  }
  return null;
}

/** Whether the target's repos still have to land before the move can be judged. */
export function isMoveTargetChecking(
  target: Project,
  targetRepos: ProjectRepo[] | undefined
): boolean {
  return target.location !== 'remote' && targetRepos === undefined;
}
