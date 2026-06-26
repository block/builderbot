/**
 * Reactive store for repo badges.
 *
 * Badges map a (githubRepo, subpath) pair to a short name and hue.
 * They are lazily generated on first encounter and cached here.
 */

import { getAllRepoBadges, ensureRepoBadges, updateRepoBadge } from '../commands';
import { agentState } from '../features/agents/agent.svelte';
import { getPreferredAgent } from '../features/settings/preferences.svelte';
import type { RepoBadge } from '../types';

function badgeKey(githubRepo: string, subpath: string): string {
  return `${githubRepo}:${subpath}`;
}

class RepoBadgeStore {
  private badges = $state<Map<string, RepoBadge>>(new Map());

  /** Return all loaded badges. */
  all(): RepoBadge[] {
    return Array.from(this.badges.values());
  }

  /** Look up a badge for a repo+subpath. Returns undefined if not yet loaded. */
  lookup(githubRepo: string, subpath: string | null | undefined): RepoBadge | undefined {
    return this.badges.get(badgeKey(githubRepo, subpath ?? ''));
  }

  /** Load all badges from the backend. Call once on app startup. */
  async loadAll(): Promise<void> {
    try {
      const all = await getAllRepoBadges();
      const next = new Map<string, RepoBadge>();
      for (const badge of all) {
        next.set(badgeKey(badge.githubRepo, badge.subpath), badge);
      }
      this.badges = next;
    } catch (e) {
      console.error('[RepoBadgeStore] Failed to load badges:', e);
    }
  }

  /** Update a badge's short name and hue, persisting to the backend. */
  async update(
    githubRepo: string,
    subpath: string | null | undefined,
    shortName: string,
    hue: number
  ): Promise<RepoBadge> {
    const sp = subpath ?? '';
    const updated = await updateRepoBadge(githubRepo, sp, shortName, hue);
    const next = new Map(this.badges);
    next.set(badgeKey(githubRepo, sp), updated);
    this.badges = next;
    return updated;
  }

  /** Remove a badge from the local cache (backend deletion is handled by removeProjectRepo). */
  remove(githubRepo: string, subpath: string | null | undefined): void {
    const key = badgeKey(githubRepo, subpath ?? '');
    if (this.badges.has(key)) {
      const next = new Map(this.badges);
      next.delete(key);
      this.badges = next;
    }
  }

  /** Ensure badges exist for the given repos. Generates missing ones. */
  async ensureForRepos(
    repos: Array<{ githubRepo: string; subpath: string | null | undefined }>
  ): Promise<void> {
    if (repos.length === 0) return;

    // Filter to repos we don't already have badges for
    const missing = repos.filter((r) => !this.badges.has(badgeKey(r.githubRepo, r.subpath ?? '')));
    if (missing.length === 0) return;

    try {
      const pairs: [string, string][] = missing.map((r) => [r.githubRepo, r.subpath ?? '']);
      const provider = getPreferredAgent(agentState.providers) ?? undefined;
      const newBadges = await ensureRepoBadges(pairs, provider);
      const next = new Map(this.badges);
      for (const badge of newBadges) {
        next.set(badgeKey(badge.githubRepo, badge.subpath), badge);
      }
      this.badges = next;
    } catch (e) {
      console.error('[RepoBadgeStore] Failed to ensure badges:', e);
    }
  }
}

export const repoBadgeStore = new RepoBadgeStore();
