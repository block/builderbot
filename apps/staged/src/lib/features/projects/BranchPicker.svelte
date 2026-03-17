<!--
  BranchPicker.svelte - PR or branch picker with search dropdown

  Features:
  - Fetches PRs and remote branches for the selected repo
  - Filters by typed query
  - Selecting a PR uses its head ref as the branch name
  - Selecting a branch uses its name directly
-->
<script lang="ts">
  import { GitPullRequest, GitBranch } from 'lucide-svelte';
  import type { PullRequest, BranchRef } from '../../types';
  import * as commands from '../../api/commands';

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
    onSelect?: (selection: BranchSelection) => void;
  }

  let { value = $bindable(''), repo, disabled = false, onSelect }: Props = $props();

  let pullRequests = $state<PullRequest[]>([]);
  let branches = $state<BranchRef[]>([]);
  let loading = $state(false);
  let showDropdown = $state(false);
  let highlightedIndex = $state(-1);
  let inputEl: HTMLInputElement | undefined = $state();

  // Fetch PRs and branches when repo changes
  $effect(() => {
    if (repo) {
      fetchData(repo);
    } else {
      pullRequests = [];
      branches = [];
    }
  });

  async function fetchData(ghRepo: string) {
    loading = true;
    try {
      const [prs, refs] = await Promise.all([
        commands.listPullRequests(ghRepo).catch(() => [] as PullRequest[]),
        commands.listGitBranches(ghRepo).catch(() => [] as BranchRef[]),
      ]);
      pullRequests = prs;
      branches = refs;
    } finally {
      loading = false;
    }
  }

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
      // Skip the default branch and any refs already covered by PRs
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
  }

  function handleFocus() {
    showDropdown = true;
  }

  function handleBlur() {
    setTimeout(() => {
      showDropdown = false;
    }, 150);
  }

  function selectItem(item: PickerItem) {
    value = item.branchName;
    showDropdown = false;
    highlightedIndex = -1;
    inputEl?.focus();

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
    <input
      bind:this={inputEl}
      class="branch-input"
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

  .branch-input {
    width: 100%;
    min-height: 42px;
    border: 1.5px solid var(--border-muted);
    background: transparent;
    color: var(--text-primary);
    border-radius: 10px;
    padding: 10px 14px;
    font-size: var(--size-md);
    font-family: inherit;
    outline: none;
    transition: border-color 0.15s ease;
    box-sizing: border-box;
  }

  .branch-input:focus {
    border-color: var(--ui-accent);
  }

  .branch-input::placeholder {
    color: var(--text-faint);
  }

  .branch-input:disabled {
    opacity: 0.6;
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
</style>
