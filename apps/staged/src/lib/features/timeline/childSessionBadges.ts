/**
 * Badge assembly for the repo sessions a running project session spawned.
 *
 * A running project session normally shows its latest agent message as a
 * subtitle. When it has repo sessions in flight, those badges say more than
 * the hint does, so they take the subtitle's place until the children finish.
 */

import type { ChildSessionInfo, RepoBadge } from '../../types';

export interface ChildSessionBadgeProps {
  /** Child session id — unique per badge, so two children in one repo keep two badges. */
  key: string;
  /** One of `'commit'`, `'note'`, `'review'`. */
  sessionType: string;
  /**
   * Repo name to read as "*type* in *shortName*". `null` when the child's
   * branch has no project repo at all, which leaves the badge as just its type.
   */
  shortName: string | null;
  /** Repo badge hue, or `null` while the badge is still being generated. */
  hue: number | null;
}

/**
 * Stand-in short name for a repo whose badge hasn't been generated yet: the
 * subpath's last segment, else the repo name after the owner. Mirrors the
 * segment the backend's `fallback_short_name` picks, without its lowercasing
 * and truncation — this is a one-off label, not a name that has to stay
 * unique against other badges.
 */
export function fallbackShortName(
  githubRepo: string | null,
  subpath: string | null
): string | null {
  const source = subpath?.trim() ? subpath : githubRepo;
  if (!source) return null;
  const segment = source.split('/').filter(Boolean).pop();
  return segment ?? null;
}

/**
 * Project running children onto badge props, tinting each with its *repo's*
 * hue rather than the session type's colour — the type is already in the
 * label, and the repo is what distinguishes one in-flight child from another.
 *
 * `lookupBadge` is the (githubRepo, subpath) → badge cache lookup; a miss
 * falls back to an untinted badge rather than dropping the child, so work in a
 * repo whose badge is still generating stays visible.
 */
export function buildChildSessionBadges(
  children: ChildSessionInfo[],
  lookupBadge: (githubRepo: string, subpath: string | null) => RepoBadge | undefined
): ChildSessionBadgeProps[] {
  return children.map((child) => {
    const badge = child.githubRepo ? lookupBadge(child.githubRepo, child.subpath) : undefined;
    return {
      key: child.sessionId,
      sessionType: child.sessionType,
      shortName: badge?.shortName ?? fallbackShortName(child.githubRepo, child.subpath),
      hue: badge?.hue ?? null,
    };
  });
}

/** The repos whose badges need generating before these children can be tinted. */
export function childSessionRepos(
  children: ChildSessionInfo[]
): Array<{ githubRepo: string; subpath: string | null }> {
  const seen = new Set<string>();
  const repos: Array<{ githubRepo: string; subpath: string | null }> = [];
  for (const child of children) {
    if (!child.githubRepo) continue;
    const key = `${child.githubRepo}:${child.subpath ?? ''}`;
    if (seen.has(key)) continue;
    seen.add(key);
    repos.push({ githubRepo: child.githubRepo, subpath: child.subpath });
  }
  return repos;
}

/**
 * The subtitle a running project-session row should show: the live agent hint,
 * unless child badges have taken the row's meta line over.
 */
export function runningSecondaryMeta(
  badges: ChildSessionBadgeProps[],
  liveHint: string | undefined
): string | undefined {
  if (badges.length > 0) return undefined;
  return liveHint ?? 'Generating note';
}
