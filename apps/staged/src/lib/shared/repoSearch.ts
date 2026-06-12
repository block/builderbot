function searchTerms(githubRepo: string, subpath: string | null | undefined): string[] {
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
  const tokens = query.toLowerCase().trim().split(/\s+/).filter(Boolean);

  if (tokens.length === 0) return true;

  const terms = searchTerms(githubRepo, subpath);
  return tokens.every((token) => terms.some((term) => term.includes(token)));
}
