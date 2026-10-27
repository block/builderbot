/**
 * Mirrors `FETCH_TTL_MS` in `src-tauri/src/git/state.rs`: within this window
 * of a previous refresh, `refresh_branch_git_state` (without `force`) skips
 * the `git fetch` and only recomputes state from local refs. Keep in sync.
 */
export const GIT_STATE_FETCH_TTL_MS = 30_000;

/**
 * Whether a layer re-activation should start a git-state refresh for a card.
 *
 * The backend TTL bounds the git work, not the round trip: without this gate
 * every card in a project pays one synchronous `refresh_branch_git_state`
 * command on each home → project return. Skip the call when this card's
 * last refresh (or the last `git-state-updated` event it merged) is still
 * inside the fetch TTL, since the fetch would be skipped anyway.
 *
 * `lastRefreshedAt` is null when the card has never refreshed.
 */
export function shouldRefreshGitStateOnActivation(
  lastRefreshedAt: number | null,
  now: number,
  ttlMs: number = GIT_STATE_FETCH_TTL_MS
): boolean {
  if (lastRefreshedAt === null) return true;
  return now - lastRefreshedAt >= ttlMs;
}
