<!--
  BranchPicker.svelte - PR or branch picker with search dropdown

  Features:
  - Fetches PRs and remote branches for the selected repo
  - Filters by typed query
  - Selecting a PR uses its head ref as the branch name
  - Selecting a branch uses its name directly
-->
<script lang="ts">
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import type { PullRequest, BranchRef } from '../../types';
  import * as commands from '../../api/commands';
  import { Input } from '$lib/components/ui/input';

  interface PickerItem {
    kind: 'pr' | 'branch';
    label: string;
    branchName: string;
    detail?: string;
  }

  export interface BranchSelection {
    kind: 'pr' | 'branch';
    branchName: string;
    /** For PRs this is the PR title; for branches it is the branch name. */
    label: string;
  }

  interface Props {
    value: string;
    repo: string;
    disabled?: boolean;
    isNewBranch?: boolean;
    /** The PR that matches the current branch value, if any. */
    matchedPr?: PullRequest | null;
    /** When set, auto-select this PR after data loads. Cleared after use. */
    initialPrNumber?: number | null;
    /** When set, auto-fill this branch name after data loads. Cleared after use. */
    initialBranchName?: string | null;
    onSelect?: (selection: BranchSelection) => void;
    /** Called when a fork PR is detected and the repo should switch to the head repo. */
    onRepoChange?: (newRepo: string) => void;
    /** Called when a pasted PR URL points to a fork and the base repo should be used instead. */
    onBaseRepoSwitch?: (baseRepo: string, forkRepo: string) => void;
  }

  let {
    value = $bindable(''),
    repo,
    disabled = false,
    isNewBranch = $bindable(false),
    matchedPr = $bindable(null),
    initialPrNumber = $bindable(null),
    initialBranchName = $bindable(null),
    onSelect,
    onRepoChange,
    onBaseRepoSwitch,
  }: Props = $props();

  let pullRequests = $state<PullRequest[]>([]);
  let branches = $state<BranchRef[]>([]);
  let loading = $state(false);
  let showDropdown = $state(false);
  let highlightedIndex = $state(-1);
  let inputEl: HTMLInputElement | null = $state(null);
  /** Generation counter to discard stale async responses when repo changes rapidly. */
  let fetchGeneration = 0;
  /** Repo slug whose data is already loaded — prevents redundant re-fetch after fork switch. */
  let loadedRepo: string | null = null;

  // Fetch PRs and branches when repo changes.
  //
  // The `loadedRepo` guard prevents an infinite re-fetch loop during fork
  // detection: when `onBaseRepoSwitch` sets `selectedRepo = baseRepo` in the
  // parent, this effect re-fires with the new repo value. But BranchPicker
  // already set `loadedRepo = parentRepo` (which equals `baseRepo`) before
  // invoking the callback, so `r === loadedRepo` short-circuits the fetch.
  $effect(() => {
    const r = repo; // read `repo` to track it as a dependency
    if (r) {
      if (r === loadedRepo) return;
      fetchData(r);
    } else {
      // Repo cleared (e.g. cancel button) — discard any in-flight fetch
      // results by bumping the generation counter alongside clearing state.
      ++fetchGeneration;
      pullRequests = [];
      branches = [];
      loadedRepo = null;
    }
  });

  async function fetchData(ghRepo: string) {
    const gen = ++fetchGeneration;
    const prNum = initialPrNumber;
    loading = true;

    try {
      // When we have an initialPrNumber, fetch the specific PR directly
      // instead of searching a potentially incomplete list. This works
      // regardless of how many open PRs the repo has and also works for
      // closed/merged PRs.
      if (prNum != null && initialPrNumber === prNum) {
        // Fetch the PR directly and check parent repo in parallel with list data.
        const [directPr, parentRepo, prs, refs] = await Promise.all([
          commands.getPrForRepo(ghRepo, prNum).catch(() => null),
          commands.getParentRepo(ghRepo).catch(() => null),
          commands.listPullRequests(ghRepo).catch(() => [] as PullRequest[]),
          commands.listGitBranches(ghRepo).catch(() => [] as BranchRef[]),
        ]);

        if (gen !== fetchGeneration) return;
        pullRequests = prs;
        branches = refs;
        loadedRepo = ghRepo;

        if (directPr && initialPrNumber === prNum) {
          // Prepend before selectItem so the PR is findable in pullRequests.
          if (!prs.some((p) => p.number === directPr.number)) {
            pullRequests = [directPr, ...prs];
          }
          selectItem(
            {
              kind: 'pr',
              label: `#${directPr.number} ${directPr.title}`,
              branchName: directPr.headRef,
              detail: directPr.headRef,
            },
            { focus: false }
          );
          initialPrNumber = null;
        } else if (parentRepo && initialPrNumber === prNum) {
          // PR not on this repo — try the parent (upstream) repo.
          const [parentPr, parentPrs, parentRefs] = await Promise.all([
            commands.getPrForRepo(parentRepo, prNum).catch(() => null),
            commands.listPullRequests(parentRepo).catch(() => [] as PullRequest[]),
            commands.listGitBranches(parentRepo).catch(() => [] as BranchRef[]),
          ]);
          if (gen !== fetchGeneration || initialPrNumber !== prNum) return;

          pullRequests = parentPrs;
          branches = parentRefs;
          loadedRepo = parentRepo;

          onBaseRepoSwitch?.(parentRepo, ghRepo);

          if (parentPr) {
            // Prepend before selectItem so the PR is findable in pullRequests.
            if (!parentPrs.some((p) => p.number === parentPr.number)) {
              pullRequests = [parentPr, ...parentPrs];
            }
            selectItem(
              {
                kind: 'pr',
                label: `#${parentPr.number} ${parentPr.title}`,
                branchName: parentPr.headRef,
                detail: parentPr.headRef,
              },
              { focus: false }
            );
          }
          initialPrNumber = null;
        } else {
          initialPrNumber = null;
        }
      }
      // No initialPrNumber — normal fetch for branches / initialBranchName.
      else {
        const [prs, refs] = await Promise.all([
          commands.listPullRequests(ghRepo).catch(() => [] as PullRequest[]),
          commands.listGitBranches(ghRepo).catch(() => [] as BranchRef[]),
        ]);

        if (gen !== fetchGeneration) return;
        pullRequests = prs;
        branches = refs;
        loadedRepo = ghRepo;

        // Auto-fill a branch name if initialBranchName was provided (e.g. from a pasted branch URL).
        // Skip focusing the input — the user didn't interact with the picker directly.
        if (initialBranchName) {
          const branchNameToFind = initialBranchName;
          // Check PRs first — a branch might back a PR
          const prForBranch = prs.find((p) => p.headRef === branchNameToFind);
          if (prForBranch) {
            selectItem(
              {
                kind: 'pr',
                label: `#${prForBranch.number} ${prForBranch.title}`,
                branchName: prForBranch.headRef,
                detail: prForBranch.headRef,
              },
              { focus: false }
            );
          } else {
            // Check remote branches
            const refName = refs.find((r) => r.name.replace(/^origin\//, '') === branchNameToFind);
            if (refName) {
              selectItem(
                {
                  kind: 'branch',
                  label: branchNameToFind,
                  branchName: branchNameToFind,
                },
                { focus: false }
              );
            } else {
              // Branch not in the list — just set the value (will show as "New branch").
              // Still check for an associated PR via --head lookup.
              value = branchNameToFind;
              onSelect?.({
                kind: 'branch',
                branchName: branchNameToFind,
                label: branchNameToFind,
              });
              if (ghRepo) {
                commands
                  .getPrForBranch(ghRepo, branchNameToFind)
                  .then((pr) => {
                    if (pr && value === branchNameToFind) {
                      if (!pullRequests.some((p) => p.number === pr.number)) {
                        pullRequests = [pr, ...pullRequests];
                      }
                      matchedPr = pr;
                      if (pr.headRepo && pr.headRepo !== repo) {
                        onRepoChange?.(pr.headRepo);
                      }
                    }
                  })
                  .catch(() => {});
              }
            }
          }
          initialBranchName = null;
        }
      }
    } finally {
      loading = false;
    }
  }

  /** All known remote branch names (without origin/ prefix). */
  let knownBranches = $derived.by((): Set<string> => {
    const names = new Set<string>();
    for (const pr of pullRequests) {
      names.add(pr.headRef);
    }
    for (const ref of branches) {
      names.add(ref.name.replace(/^origin\//, ''));
    }
    return names;
  });

  // Update isNewBranch whenever value or known branches change.
  // Gate on !loading to avoid a flash of "New branch" while data is being fetched.
  $effect(() => {
    const trimmed = value.trim();
    isNewBranch = trimmed.length > 0 && !loading && !knownBranches.has(trimmed);
  });

  /** Build a unified list of picker items from PRs and branches. */
  let allItems = $derived.by((): PickerItem[] => {
    const items: PickerItem[] = [];

    for (const pr of pullRequests) {
      items.push({
        kind: 'pr',
        label: `#${pr.number} ${pr.title}`,
        branchName: pr.headRef,
        detail: pr.headRef,
      });
    }

    for (const ref of branches) {
      // Strip "origin/" prefix for display
      const name = ref.name.replace(/^origin\//, '');
      // Skip any refs already covered by PRs (avoids duplicate entries)
      if (items.some((i) => i.branchName === name)) continue;
      items.push({
        kind: 'branch',
        label: name,
        branchName: name,
      });
    }

    return items;
  });

  /** Filter items by the current input value. */
  let filteredItems = $derived.by((): PickerItem[] => {
    const query = value.trim().toLowerCase();
    if (!query) return allItems.slice(0, 50);
    return allItems
      .filter(
        (item) =>
          item.label.toLowerCase().includes(query) || item.branchName.toLowerCase().includes(query)
      )
      .slice(0, 50);
  });

  function handleInput() {
    highlightedIndex = -1;
    showDropdown = true;
    // User is typing freely — clear any previously matched PR
    matchedPr = null;
  }

  function handleFocus() {
    showDropdown = true;
  }

  function handleBlur() {
    setTimeout(() => {
      showDropdown = false;
    }, 150);
  }

  function selectItem(item: PickerItem, { focus = true }: { focus?: boolean } = {}) {
    value = item.branchName;
    showDropdown = false;
    highlightedIndex = -1;
    if (focus) inputEl?.focus();

    // Update matched PR: set when selecting a PR item, clear for branches
    if (item.kind === 'pr') {
      matchedPr = pullRequests.find((pr) => pr.headRef === item.branchName) ?? null;
      // For fork PRs, signal repo change so the displayed repo updates
      if (matchedPr?.headRepo && matchedPr.headRepo !== repo) {
        onRepoChange?.(matchedPr.headRepo);
      }
    } else {
      matchedPr = null;
      // The branch might have a PR outside the top-50 list — look it up by head ref.
      if (repo) {
        const branchName = item.branchName;
        const gen = fetchGeneration;
        commands
          .getPrForBranch(repo, branchName)
          .then((pr) => {
            // Only apply if the user hasn't changed selection or repo since we fired.
            if (pr && value === branchName && gen === fetchGeneration) {
              if (!pullRequests.some((p) => p.number === pr.number)) {
                pullRequests = [pr, ...pullRequests];
              }
              matchedPr = pr;
              if (pr.headRepo && pr.headRepo !== repo) {
                onRepoChange?.(pr.headRepo);
              }
            }
          })
          .catch(() => {
            /* no PR for this branch — that's fine */
          });
      }
    }

    // For PRs, pass the PR title (strip the "#123 " prefix); for branches, pass the branch name.
    const label = item.kind === 'pr' ? item.label.replace(/^#\d+\s*/, '') : item.branchName;
    onSelect?.({ kind: item.kind, branchName: item.branchName, label });
  }

  function handleKeydown(e: KeyboardEvent) {
    if (!showDropdown || filteredItems.length === 0) return;

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlightedIndex = Math.min(highlightedIndex + 1, filteredItems.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightedIndex = Math.max(highlightedIndex - 1, -1);
    } else if (e.key === 'Enter' && highlightedIndex >= 0) {
      e.preventDefault();
      e.stopPropagation();
      selectItem(filteredItems[highlightedIndex]);
    } else if (e.key === 'Escape') {
      showDropdown = false;
      highlightedIndex = -1;
    } else if (e.key === 'Tab' && highlightedIndex >= 0) {
      e.preventDefault();
      selectItem(filteredItems[highlightedIndex]);
    }
  }
</script>

<div class="branch-picker-wrapper">
  <div class="input-container">
    <Input
      bind:ref={inputEl}
      class="min-h-[42px] rounded-[10px] bg-background px-3.5 py-2.5 text-base"
      type="text"
      bind:value
      oninput={handleInput}
      onfocus={handleFocus}
      onblur={handleBlur}
      onkeydown={handleKeydown}
      id="project-branch"
      placeholder="Search PRs or branches…"
      autocomplete="off"
      autocorrect="off"
      autocapitalize="off"
      spellcheck={false}
      {disabled}
    />
  </div>

  {#if showDropdown && filteredItems.length > 0 && !disabled}
    <div class="suggestions-dropdown">
      {#each filteredItems as item, i}
        <button
          class="suggestion-item"
          class:highlighted={i === highlightedIndex}
          onmousedown={(e) => {
            e.preventDefault();
            selectItem(item);
          }}
        >
          {#if item.kind === 'pr'}
            <GitPullRequest size={14} />
          {:else}
            <GitBranch size={14} />
          {/if}
          <span class="suggestion-label">{item.label}</span>
          {#if item.detail}
            <span class="suggestion-detail">{item.detail}</span>
          {/if}
        </button>
      {/each}
    </div>
  {:else if showDropdown && loading && !disabled}
    <div class="suggestions-dropdown">
      <div class="loading-hint">Loading…</div>
    </div>
  {/if}

  {#if matchedPr}
    <div class="pr-info-card">
      <GitPullRequest size={14} class="pr-info-icon" />
      <div class="pr-info-content">
        <span class="pr-info-title">#{matchedPr.number} {matchedPr.title}</span>
        <span class="pr-info-meta"
          >{matchedPr.author} · {matchedPr.baseRef}{#if matchedPr.draft}
            · Draft{/if}</span
        >
      </div>
    </div>
  {/if}
</div>

<style>
  .branch-picker-wrapper {
    position: relative;
  }

  .input-container {
    position: relative;
    display: flex;
    align-items: center;
  }

  .suggestions-dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    margin-top: 4px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    max-height: 200px;
    overflow-y: auto;
    z-index: 100;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.15);
  }

  .suggestion-item {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 8px 12px;
    border: none;
    background: transparent;
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-family: inherit;
    cursor: pointer;
    text-align: left;
    transition: background-color 0.1s ease;
  }

  .suggestion-item:hover,
  .suggestion-item.highlighted {
    background: var(--bg-hover);
  }

  .suggestion-item :global(svg) {
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .suggestion-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .suggestion-detail {
    margin-left: auto;
    font-size: var(--size-xs);
    color: var(--text-faint);
    flex-shrink: 0;
  }

  .loading-hint {
    padding: 8px 12px;
    font-size: var(--size-sm);
    color: var(--text-faint);
  }

  .pr-info-card {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    margin-top: 6px;
    padding: 8px 12px;
    background: var(--bg-hover);
    border-radius: 8px;
  }

  .pr-info-card :global(.pr-info-icon) {
    color: var(--text-faint);
    flex-shrink: 0;
    margin-top: 1px;
  }

  .pr-info-content {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .pr-info-title {
    font-size: var(--size-sm);
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pr-info-meta {
    font-size: var(--size-xs);
    color: var(--text-faint);
  }
</style>
