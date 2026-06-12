import type { ActionContext } from '../../api/commands';
import { matchesRepoSearch } from '../../shared/repoSearch';

export { matchesRepoSearch };

export function matchesRepoContextSearch(context: ActionContext, query: string): boolean {
  return matchesRepoSearch(context.githubRepo, context.subpath, query);
}
