/**
 * Per-window title, derived from the window's active project filters.
 *
 * Every window is built from the same `main` entry in `tauri.conf.json`, which
 * hard-codes `"title": "Staged"`, and nothing ever changed it — so the macOS
 * Window submenu, which labels each entry from `NSWindow.title`, read
 * "Staged / Staged / Staged" with three windows open. Naming each window after
 * what it is filtered down to makes those entries tell the windows apart.
 *
 * `projectFiltersStore` is module-scoped and every window is its own webview
 * with its own module graph, so the filter selection is already window-local —
 * no plumbing is needed to keep the title window-local too. Filters are not
 * persisted, so a cold window always starts at DEFAULT_WINDOW_TITLE and there
 * is no restore path.
 *
 * On macOS the title is *purely* a Window-menu label: the conf also sets
 * `titleBarStyle: "Overlay"` and `hiddenTitle: true`, so nothing draws it.
 * Both keys are macOS-only, so on Windows/Linux the title lands in the native
 * titlebar instead — accepted deliberately: a titlebar naming the current
 * filter is informative and matches how ordinary apps title windows.
 */

import { getWindowSync } from '../../transport';
import { repoEmphasis } from '../../shared/repoLabel';
import { parseRepoFilterKey, type RepoFilterRef } from '../projects/projectFilters.svelte';

/** The title of an unfiltered window — and what the conf already sets. */
export const DEFAULT_WINDOW_TITLE = 'Staged';

/** Repo filters named before the rest collapse into "+N more". */
const MAX_REPOS_NAMED = 2;

/**
 * The window title for a filter set, mirroring the sidebar summary's ordering
 * so the two surfaces say the same thing: status filters first (in a fixed
 * order, not selection order), then the repo filters.
 *
 * Repos are named by their emphasised segment only (see repoLabel.ts) and
 * never by `repoBadgeStore` short names: badges load asynchronously, so a
 * badge-derived title would visibly flip mid-session. Deriving from the filter
 * keys alone keeps this a pure function of the filter set — which also means a
 * stale filter naming a repo no longer in any project still titles the window,
 * exactly as `activeRepoFilters` still summarises it in the sidebar.
 */
export function formatWindowTitle(activeFilters: Set<string>): string {
  const segments: string[] = [];
  if (activeFilters.has('unread')) segments.push('Unread');
  if (activeFilters.has('running')) segments.push('Running');

  // Sorted by the rendered label rather than the full path, so the title reads
  // alphabetically as displayed. Two repos can collapse to the same segment;
  // the sidebar chips have that property too, and disambiguating back to full
  // paths is complexity a truncating menu item doesn't repay.
  const repos = [...activeFilters]
    .map(parseRepoFilterKey)
    .filter((ref): ref is RepoFilterRef => ref !== null)
    .map((ref) => repoEmphasis(ref))
    .sort((a, b) => a.localeCompare(b));

  if (repos.length > 0) {
    const named = repos.slice(0, MAX_REPOS_NAMED);
    if (repos.length > named.length) named.push(`+${repos.length - named.length} more`);
    segments.push(named.join(', '));
  }

  return segments.length > 0 ? segments.join(' · ') : DEFAULT_WINDOW_TITLE;
}

/** The title last requested — the window opens with the conf's default. */
let requestedTitle = DEFAULT_WINDOW_TITLE;
let titleGeneration = 0;
let inFlight: Promise<void> = Promise.resolve();

/**
 * Push a title to the current window (or, in web mode, the browser tab).
 *
 * Two guards, since this is driven by an effect over a churning filter set:
 * unchanged titles issue no IPC at all (including the effect's first run,
 * which always recomputes the default the window already has), and calls are
 * serialized behind the previous one with a generation check after the await,
 * so rapid toggling can't let a superseded title land last.
 */
export function applyWindowTitle(title: string): Promise<void> {
  if (title === requestedTitle) return inFlight;
  requestedTitle = title;

  const generation = ++titleGeneration;
  inFlight = inFlight.then(async () => {
    if (generation !== titleGeneration) return;
    try {
      await getWindowSync().setTitle(title);
    } catch (error) {
      console.warn('[windowTitle] Failed to set the window title:', error);
    }
  });
  return inFlight;
}
