import { matchesAllTerms, queryTerms } from './textSearch';

/**
 * The lower-cased fields a repo is searched by: the full `owner/repo`, its
 * owner and name, the subpath and each of its segments, and the repo joined
 * with its subpath both ways ("owner/repo/sub", "repo/sub").
 */
export function searchTerms(githubRepo: string, subpath: string | null | undefined): string[] {
  const repo = githubRepo.toLowerCase();
  const [owner = '', repoName = ''] = repo.split('/');
  const subpathParts = subpath?.toLowerCase().split('/').filter(Boolean) ?? [];
  const normalizedSubpath = subpathParts.join('/');

  return [
    repo,
    owner,
    repoName,
    normalizedSubpath,
    ...subpathParts,
    normalizedSubpath ? `${repo}/${normalizedSubpath}` : '',
    normalizedSubpath && repoName ? `${repoName}/${normalizedSubpath}` : '',
  ].filter(Boolean);
}

export function matchesRepoSearch(
  githubRepo: string,
  subpath: string | null | undefined,
  query: string
): boolean {
  return matchesAllTerms(queryTerms(query), ...searchTerms(githubRepo, subpath));
}
