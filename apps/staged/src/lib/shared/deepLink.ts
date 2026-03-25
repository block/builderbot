/**
 * Convert a `staged:` deep-link URL into an `https:` GitHub URL.
 *
 * The user types `staged://github.com/owner/repo/pull/123` in their browser
 * (replacing `https` with `staged`). We strip the scheme and reconstruct
 * the original `https://…` URL so it can be fed into the existing
 * `parseGitHubUrl` helper.
 *
 * Returns `null` if the URL doesn't look like a GitHub URL.
 */
export function convertDeepLinkToHttps(raw: string): string | null {
  // The URL arrives as "staged://github.com/…" or "staged:github.com/…".
  // Strip the scheme portion to get the rest.
  const withoutScheme = raw.replace(/^staged:\/?\/?/, '');

  // Only allow GitHub URLs for now.
  if (!withoutScheme.startsWith('github.com/')) {
    return null;
  }

  return `https://${withoutScheme}`;
}
