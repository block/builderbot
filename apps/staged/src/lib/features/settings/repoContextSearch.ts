import type { ActionContext } from '../../api/commands';

function searchTerms(githubRepo: string, subpath: string | null | undefined): string[] {
  const repo = githubRepo.toLowerCase();
  const [org = '', repoName = ''] = repo.split('/');
  const sub = subpath?.toLowerCase() ?? '';
  const subpathParts = sub.split('/').filter(Boolean);

  return [repo, org, repoName, sub, ...subpathParts].filter(Boolean);
}

export function matchesRepoContextSearch(context: ActionContext, query: string): boolean {
  return matchesRepoSearch(context.githubRepo, context.subpath, query);
}

export function matchesRepoSearch(
  githubRepo: string,
  subpath: string | null | undefined,
  query: string
): boolean {
  const tokens = query.toLowerCase().trim().split(/\s+/).filter(Boolean);

  if (tokens.length === 0) return true;

  const terms = searchTerms(githubRepo, subpath);
  return tokens.every((token) => terms.some((term) => term.includes(token)));
}
