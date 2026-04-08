import type { ActionContext } from '../../api/commands';

function searchTerms(context: ActionContext): string[] {
  const githubRepo = context.githubRepo.toLowerCase();
  const [org = '', repoName = ''] = githubRepo.split('/');
  const subpath = context.subpath?.toLowerCase() ?? '';

  return [githubRepo, org, repoName, subpath].filter(Boolean);
}

export function matchesRepoContextSearch(context: ActionContext, query: string): boolean {
  const tokens = query.toLowerCase().trim().split(/\s+/).filter(Boolean);

  if (tokens.length === 0) return true;

  const terms = searchTerms(context);
  return tokens.every((token) => terms.some((term) => term.includes(token)));
}
