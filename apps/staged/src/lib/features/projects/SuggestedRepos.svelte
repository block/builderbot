<!--
  SuggestedRepos.svelte - Shows repos that have historically been used
  alongside the current project's repos, with an "Add" button for each.

  Rendered as a dotted round rect below the branch cards, with repo-badge-style
  chips showing the full repo+subpath name. A dismiss button persists the
  hidden state across app launches.
-->
<script lang="ts">
  import { Plus, X } from 'lucide-svelte';
  import type { Project, ProjectRepo, SuggestedRepo } from '../../types';
  import type { RepoSelection as RepoPickerSelection } from '../../shared/githubUrl';
  import * as commands from '../../api/commands';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { getStoreValue, setStoreValue } from '../../shared/persistentStore';

  interface Props {
    project: Project;
    reposById: Map<string, ProjectRepo>;
    onRepoSelected?: (selection: RepoPickerSelection) => void | Promise<void>;
  }

  let { project, reposById, onRepoSelected }: Props = $props();

  let suggestions = $state<SuggestedRepo[]>([]);
  let fetchGeneration = 0;
  let addingKey = $state<string | null>(null);
  let dismissed = $state(false);
  let dismissLoaded = $state(false);

  const DISMISS_STORE_KEY = 'suggested-repos-dismissed';

  // Load dismiss state on mount.
  $effect(() => {
    void project.id; // re-check when project changes
    getStoreValue<Record<string, boolean>>(DISMISS_STORE_KEY).then((val) => {
      dismissed = val?.[project.id] ?? false;
      dismissLoaded = true;
    });
  });

  // Build a set of already-attached repo keys for filtering (current project only).
  let attachedKeys = $derived(
    new Set(
      [...reposById.values()]
        .filter((r) => r.projectId === project.id)
        .map((r) => {
          const sub = r.subpath?.trim();
          return sub ? `${r.githubRepo}::${sub}` : r.githubRepo;
        })
    )
  );

  async function fetchSuggestions() {
    const gen = ++fetchGeneration;
    try {
      const result = await commands.getSuggestedRepos(project.id);
      if (gen !== fetchGeneration) return; // discard stale response
      // Filter out any that have been attached since we last fetched.
      suggestions = result.filter((s) => {
        const key = s.subpath ? `${s.githubRepo}::${s.subpath}` : s.githubRepo;
        return !attachedKeys.has(key);
      });
      // Ensure badges exist for suggestions so we can look up hues.
      if (suggestions.length > 0) {
        await repoBadgeStore.ensureForRepos(
          suggestions.map((s) => ({ githubRepo: s.githubRepo, subpath: s.subpath }))
        );
      }
    } catch {
      if (gen !== fetchGeneration) return;
      suggestions = [];
    }
  }

  // Re-fetch when the set of attached repos changes.
  $effect(() => {
    void attachedKeys.size;
    fetchSuggestions();
  });

  function suggestionKey(s: SuggestedRepo): string {
    return s.subpath ? `${s.githubRepo}::${s.subpath}` : s.githubRepo;
  }

  async function handleAdd(suggestion: SuggestedRepo) {
    const key = suggestionKey(suggestion);
    addingKey = key;
    try {
      await onRepoSelected?.({
        nameWithOwner: suggestion.githubRepo,
        subpath: suggestion.subpath ?? undefined,
      });
    } finally {
      addingKey = null;
    }
  }

  async function handleDismiss() {
    dismissed = true;
    const current = (await getStoreValue<Record<string, boolean>>(DISMISS_STORE_KEY)) ?? {};
    await setStoreValue(DISMISS_STORE_KEY, { ...current, [project.id]: true });
  }

  function badgeHue(suggestion: SuggestedRepo): number {
    return repoBadgeStore.lookup(suggestion.githubRepo, suggestion.subpath)?.hue ?? 210;
  }

  function badgeBg(hue: number): string {
    return darkMode.value ? `hsl(${hue} 35% 22%)` : `hsl(${hue} 50% 92%)`;
  }

  function badgeFg(hue: number): string {
    return darkMode.value ? `hsl(${hue} 50% 75%)` : `hsl(${hue} 55% 35%)`;
  }

  function badgeBorder(hue: number): string {
    return darkMode.value ? `hsl(${hue} 35% 32%)` : `hsl(${hue} 45% 82%)`;
  }

  function badgeBgHover(hue: number): string {
    return darkMode.value ? `hsl(${hue} 40% 30%)` : `hsl(${hue} 60% 85%)`;
  }

  function badgeBorderHover(hue: number): string {
    return darkMode.value ? `hsl(${hue} 45% 45%)` : `hsl(${hue} 50% 65%)`;
  }
</script>

{#if dismissLoaded && !dismissed && suggestions.length > 0}
  <div class="suggested-repos">
    <div class="suggested-header">
      <span class="suggested-title">Suggested repos</span>
      <button class="dismiss-btn" onclick={handleDismiss} title="Dismiss suggestions">
        <X size={14} />
      </button>
    </div>
    <div class="suggested-list">
      {#each suggestions as suggestion (suggestionKey(suggestion))}
        {@const hue = badgeHue(suggestion)}
        {@const isAdding = addingKey === suggestionKey(suggestion)}
        <button
          class="suggested-chip"
          style="--chip-bg: {badgeBg(hue)}; --chip-bg-hover: {badgeBgHover(
            hue
          )}; --chip-border: {badgeBorder(hue)}; --chip-border-hover: {badgeBorderHover(
            hue
          )}; --chip-fg: {badgeFg(hue)};"
          disabled={isAdding}
          onclick={() => handleAdd(suggestion)}
        >
          {#if isAdding}
            <Spinner size={13} />
          {:else}
            <Plus size={13} />
          {/if}
          <span class="chip-name">
            <RepoLabel githubRepo={suggestion.githubRepo} subpath={suggestion.subpath} />
          </span>
        </button>
      {/each}
    </div>
  </div>
{/if}

<style>
  .suggested-repos {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px;
    border: 1.5px dashed var(--border-muted);
    border-radius: 10px;
  }

  .suggested-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .suggested-title {
    color: var(--text-muted);
    font-size: var(--size-xs);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .dismiss-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 4px;
    background: transparent;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.15s,
      background 0.15s;
  }

  .dismiss-btn:hover {
    color: var(--text-secondary);
    background: var(--bg-hover);
  }

  .suggested-list {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .suggested-chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 10px;
    border: 1px solid var(--chip-border);
    border-radius: 5px;
    background: var(--chip-bg);
    color: var(--chip-fg);
    font-size: 12.5px;
    font-weight: 600;
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    line-height: 1.4;
    cursor: pointer;
    transition:
      background 0.15s,
      border-color 0.15s;
  }

  .suggested-chip:hover:not(:disabled) {
    background: var(--chip-bg-hover);
    border-color: var(--chip-border-hover);
  }

  .suggested-chip:disabled {
    cursor: default;
    opacity: 0.7;
  }

  .chip-name {
    display: inline-flex;
    max-width: 250px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Override RepoLabel colors to inherit from badge chip */
  .chip-name :global(.repo-label-prefix) {
    color: inherit;
    opacity: 0.6;
  }

  .chip-name :global(.repo-label-emphasis) {
    color: inherit;
  }
</style>
