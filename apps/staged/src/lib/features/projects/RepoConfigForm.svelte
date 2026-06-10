<!--
  RepoConfigForm.svelte - Repo configuration form fields

  Contains repository search, selected repo display, recent repos list,
  subpath input with monorepo detection, and branch/PR picker.
  Extracted from NewProjectForm for reuse in Add Repo flows.
-->
<script lang="ts" module>
  export type { BranchSelection } from './BranchPicker.svelte';
</script>

<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { slide, fade } from 'svelte/transition';
  import GitBranch from '@lucide/svelte/icons/git-branch';
  import X from '@lucide/svelte/icons/x';
  import Plus from '@lucide/svelte/icons/plus';
  import Command from '@lucide/svelte/icons/command';
  import type { RecentRepo, PullRequest } from '../../types';
  import * as commands from '../../api/commands';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import * as badge from '../../shared/badgeColors';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoSearchInput from './RepoSearchInput.svelte';
  import SubpathInput from './SubpathInput.svelte';
  import type { SubpathInputApi } from './SubpathInput.svelte';
  import BranchPicker, { type BranchSelection } from './BranchPicker.svelte';
  import type { RepoSelection } from '../../shared/githubUrl';
  import { viewport } from '../../shared/viewport.svelte';
  import { Label } from '$lib/components/ui/label';
  import { Input } from '$lib/components/ui/input';
  import { Button } from '$lib/components/ui/button';

  interface Props {
    // Bindable state
    selectedRepo?: string | null;
    /** For fork PRs, the head (fork) repo slug that differs from selectedRepo. */
    headRepo?: string | null;
    subpath?: string;
    branchName?: string;
    isNewBranch?: boolean;
    matchedPr?: PullRequest | null;
    /** Pre-fetched default branch for the selected repo (avoids slow API call during project creation). */
    defaultBranch?: string | null;

    // Config
    disabled?: boolean;
    excludeRepos?: Set<string>;
    autofocus?: boolean;

    // Optional callbacks
    onBranchSelected?: (selection: BranchSelection) => void;

    // Show "Required" badge on Repository label (for remote location in NewProjectForm)
    repoRequired?: boolean;

    // Exposed API for parent validation and programmatic repo selection
    api?: {
      waitForSubpathValidation: () => Promise<boolean>;
      selectRepo: (selection: RepoSelection) => void;
      reset: () => void;
    };
  }

  let {
    selectedRepo = $bindable(null),
    headRepo = $bindable(null),
    subpath = $bindable(''),
    branchName = $bindable(''),
    isNewBranch = $bindable(false),
    matchedPr = $bindable(null),
    defaultBranch = $bindable(null),
    disabled = false,
    excludeRepos,
    autofocus = false,
    repoRequired = false,
    onBranchSelected,
    api = $bindable<
      | {
          waitForSubpathValidation: () => Promise<boolean>;
          selectRepo: (selection: RepoSelection) => void;
          reset: () => void;
        }
      | undefined
    >(undefined),
  }: Props = $props();

  let isMonorepo = $state(false);
  let checkingMonorepo = $state(false);
  let subpathApi = $state<SubpathInputApi | undefined>(undefined);
  let recentRepos = $state<RecentRepo[]>([]);
  const SLIDE_DURATION = 150;

  /** BranchPicker is expensive to mount in WebKit — defer it until after the
   *  initial layout change paints so the UI feels responsive. */
  let showBranchPicker = $state(false);
  let branchPickerTimer: ReturnType<typeof setTimeout> | null = null;

  let pendingPrNumber = $state<number | null>(null);
  let pendingBranchName = $state<string | null>(null);

  /** For fork PRs, the head repo differs from the base repo. We display the
   *  head repo name but keep selectedRepo as the base repo for all API calls
   *  (monorepo check, default branch, subpath, project creation) since
   *  refs/pull/<N>/head only exists on the base repo's clone. */
  let displayRepo = $state<string | null>(null);

  /** True while resolving a PR from a pasted URL (fetching PRs, detecting forks).
   *  Set explicitly in handleRepoSelected; cleared explicitly when BranchPicker
   *  finishes (pendingPrNumber transitions from non-null to null) or on cancel. */
  let resolvingPr = $state(false);
  let prevPendingPrNumber: number | null = null;

  // Clear resolvingPr when BranchPicker signals completion by nulling pendingPrNumber.
  // This avoids the old derived approach where a stale matchedPr from a previous
  // selection could prematurely clear the flag.
  $effect(() => {
    const cur = pendingPrNumber;
    if (prevPendingPrNumber != null && cur == null) {
      resolvingPr = false;
    }
    prevPendingPrNumber = cur;
  });

  onMount(async () => {
    try {
      recentRepos = await commands.listRecentRepos(9);
    } catch {
      // Silently ignore — recents are a convenience, not critical
    }
  });

  onDestroy(() => {
    if (branchPickerTimer) clearTimeout(branchPickerTimer);
  });

  async function checkIfMonorepo(repo: string) {
    if (!repo) {
      isMonorepo = false;
      return;
    }

    checkingMonorepo = true;
    try {
      const moduleCount = await commands.checkMonorepoModules(repo);
      isMonorepo = moduleCount >= 20;
    } catch (e) {
      isMonorepo = false;
    } finally {
      checkingMonorepo = false;
    }
  }

  $effect(() => {
    if (selectedRepo) {
      checkIfMonorepo(selectedRepo);
      prefetchDefaultBranch(selectedRepo);
    } else {
      isMonorepo = false;
      defaultBranch = null;
    }
  });

  async function prefetchDefaultBranch(repo: string) {
    try {
      const branch = await commands.detectDefaultBranch(repo);
      if (selectedRepo === repo) defaultBranch = branch;
    } catch {
      if (selectedRepo === repo) defaultBranch = null;
    }
  }

  /** Clear all form state, including internal-only fields the parent can't bind.
   *  Mirrors the Clear button's reset plus the deferred-picker/PR-resolution state. */
  function reset() {
    if (branchPickerTimer) clearTimeout(branchPickerTimer);
    selectedRepo = null;
    headRepo = null;
    subpath = '';
    branchName = '';
    isNewBranch = false;
    matchedPr = null;
    defaultBranch = null;
    displayRepo = null;
    isMonorepo = false;
    checkingMonorepo = false;
    showBranchPicker = false;
    resolvingPr = false;
    pendingPrNumber = null;
    pendingBranchName = null;
  }

  // Expose validation API to parent
  $effect(() => {
    api = {
      waitForSubpathValidation: subpathApi
        ? () => subpathApi!.waitForValidation()
        : () => Promise.resolve(true),
      selectRepo: handleRepoSelected,
      reset,
    };
  });

  let filteredRecentRepos = $derived(
    excludeRepos
      ? recentRepos.filter((r) => !excludeRepos.has(`${r.githubRepo}\x00${r.subpath ?? ''}`))
      : recentRepos
  );

  const dark = $derived(darkMode.value);

  $effect(() => {
    if (recentRepos.length > 0) {
      repoBadgeStore.ensureForRepos(
        recentRepos.map((r) => ({ githubRepo: r.githubRepo, subpath: r.subpath }))
      );
    }
  });

  function repoHue(r: RecentRepo): number {
    return repoBadgeStore.lookup(r.githubRepo, r.subpath)?.hue ?? 210;
  }

  function handleRepoSelected(selection: RepoSelection) {
    if (branchPickerTimer) clearTimeout(branchPickerTimer);
    showBranchPicker = false;
    matchedPr = null;
    selectedRepo = selection.nameWithOwner;
    displayRepo = selection.nameWithOwner;
    subpath = selection.subpath ?? '';
    pendingPrNumber = selection.prNumber ?? null;
    pendingBranchName = selection.branchName ?? null;
    resolvingPr = selection.prNumber != null;
    // Mount BranchPicker after the slide animation completes.
    // BranchPicker is expensive to mount in WebKit, so deferring it
    // keeps the initial repo selection feeling responsive.
    branchPickerTimer = setTimeout(() => {
      showBranchPicker = true;
    }, SLIDE_DURATION + 50);
  }

  function handleKeydown(e: KeyboardEvent) {
    // ⌘1-⌘9: select a recent repo
    if (e.metaKey && e.key >= '1' && e.key <= '9') {
      const idx = parseInt(e.key) - 1;
      if (idx < filteredRecentRepos.length) {
        e.preventDefault();
        const recent = filteredRecentRepos[idx];
        handleRepoSelected({
          nameWithOwner: recent.githubRepo,
          subpath: recent.subpath ?? undefined,
        });
      }
      return;
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- Grid stacking: both the overlay and form occupy the same cell so the form
     always contributes to layout height, avoiding a jump when the overlay disappears. -->
<div class="repo-fields-stack">
  {#if resolvingPr}
    <div class="pr-loading-overlay" transition:fade={{ duration: 100 }}>
      <Spinner size={20} />
      <span class="pr-loading-text">Loading PR&hellip;</span>
      <Button
        variant="outline"
        size="xs"
        onclick={() => {
          resolvingPr = false;
          pendingPrNumber = null;
          if (branchPickerTimer) clearTimeout(branchPickerTimer);
          showBranchPicker = false;
          selectedRepo = null;
          displayRepo = null;
          headRepo = null;
          subpath = '';
          branchName = '';
          matchedPr = null;
          pendingBranchName = null;
        }}
      >
        Cancel
      </Button>
    </div>
  {/if}

  <!-- Keep form fields in the DOM so BranchPicker can resolve the PR, but hide
     them while the loading overlay is visible. -->
  <div class="repo-fields" class:resolving-hidden={resolvingPr}>
    <div class="form-group">
      <Label for="project-repo-select" class="text-muted-foreground text-xs">
        Repository
        {#if repoRequired && !selectedRepo}
          <span class="field-badge required">Required</span>
        {/if}
      </Label>
      {#if selectedRepo}
        <div class="repo-info" class:disabled>
          <GitBranch size={14} class="repo-info-icon" />
          <div class="repo-details">
            <span class="repo-name">{displayRepo ?? selectedRepo}</span>
          </div>
          <Button
            variant="ghost"
            size="icon-sm"
            class="size-7 text-[var(--bg-chrome)] hover:bg-black/10 hover:text-[var(--bg-deepest)] [&_svg]:!size-3.5"
            onclick={() => {
              if (branchPickerTimer) clearTimeout(branchPickerTimer);
              showBranchPicker = false;
              selectedRepo = null;
              displayRepo = null;
              headRepo = null;
              subpath = '';
              branchName = '';
              matchedPr = null;
              pendingPrNumber = null;
              pendingBranchName = null;
            }}
          >
            <X size={14} />
          </Button>
        </div>
      {:else}
        <RepoSearchInput onSelect={handleRepoSelected} {disabled} {excludeRepos} {autofocus} />
      {/if}

      {#if !selectedRepo && filteredRecentRepos.length > 0}
        <div class="recent-repos" out:slide={{ duration: SLIDE_DURATION }}>
          {#each filteredRecentRepos.slice(0, 5) as recent, i}
            {@const hue = repoHue(recent)}
            <button
              class="recent-chip"
              style="--chip-bg: {badge.badgeBg(hue, dark)}; --chip-bg-hover: {badge.badgeBgHover(
                hue,
                dark
              )}; --chip-border: {badge.badgeBorder(
                hue,
                dark
              )}; --chip-border-hover: {badge.badgeBorderHover(
                hue,
                dark
              )}; --chip-fg: {badge.badgeFg(hue, dark)}; --chip-shortcut-bg: {badge.badgeShortcutBg(
                hue,
                dark
              )};"
              onclick={() =>
                handleRepoSelected({
                  nameWithOwner: recent.githubRepo,
                  subpath: recent.subpath ?? undefined,
                })}
            >
              <Plus size={13} />
              <span class="chip-name">
                <RepoLabel githubRepo={recent.githubRepo} subpath={recent.subpath} />
              </span>
              {#if viewport.showShortcutHints}
                <span class="recent-chip-shortcut">
                  <Command size={9} />
                  {i + 1}
                </span>
              {/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    {#if selectedRepo}
      <div class="form-group" transition:slide={{ duration: SLIDE_DURATION }}>
        <Label for="project-subpath" class="text-muted-foreground text-xs">
          Subpath
          <span class="field-badge {isMonorepo ? 'recommended' : 'optional'}"
            >{isMonorepo ? 'Recommended' : 'Optional'}</span
          >
        </Label>
        <SubpathInput bind:value={subpath} repo={selectedRepo} {disabled} bind:api={subpathApi} />
      </div>

      <div class="form-group" transition:slide={{ duration: SLIDE_DURATION }}>
        <Label for="project-branch" class="text-muted-foreground text-xs">
          PR or Branch
          <span class="field-badge {isNewBranch ? 'new-branch' : 'optional'}"
            >{isNewBranch ? 'New branch' : 'Optional'}</span
          >
        </Label>
        {#if showBranchPicker}
          <BranchPicker
            bind:value={branchName}
            bind:isNewBranch
            bind:matchedPr
            bind:initialPrNumber={pendingPrNumber}
            bind:initialBranchName={pendingBranchName}
            repo={selectedRepo}
            {disabled}
            onSelect={onBranchSelected}
            onRepoChange={(newRepo) => {
              displayRepo = newRepo;
              headRepo = newRepo !== selectedRepo ? newRepo : null;
            }}
            onBaseRepoSwitch={(baseRepo, forkRepo) => {
              // A pasted fork URL needs the base repo for API calls/cloning.
              // Switch selectedRepo to the base repo (triggers PR re-fetch)
              // while keeping the fork as the display/head repo.
              displayRepo = forkRepo;
              headRepo = forkRepo;
              selectedRepo = baseRepo;
            }}
          />
        {:else}
          <Input
            type="text"
            placeholder="Search PRs or branches…"
            readonly
            tabindex={-1}
            class="min-h-[42px] rounded-[10px] bg-background px-3.5 py-2.5 text-base"
          />
        {/if}
      </div>
    {/if}
  </div>

  <!-- /resolving-hidden wrapper -->
</div>

<!-- /repo-fields-stack -->

<style>
  .repo-fields-stack {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    min-width: 0;
  }

  .repo-fields-stack > * {
    grid-area: 1 / 1;
  }

  .pr-loading-overlay {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 32px 16px;
    border-radius: 10px;
    background: var(--bg-surface);
    border: 1px solid var(--border-subtle);
    z-index: 1;
  }

  .pr-loading-text {
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .repo-fields {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .resolving-hidden {
    opacity: 0;
    pointer-events: none;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .field-badge {
    display: inline-block;
    font-size: 9px;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 3px;
    margin-left: 6px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .field-badge.optional {
    background-color: var(--bg-hover);
    color: var(--text-faint);
  }

  .field-badge.recommended {
    background-color: var(--ui-accent);
    color: var(--bg-deepest);
  }

  .field-badge.required {
    background-color: var(--ui-danger);
    color: var(--bg-deepest);
  }

  .field-badge.new-branch {
    background-color: var(--ui-accent);
    color: var(--bg-deepest);
  }

  .repo-info {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 42px;
    padding: 10px 8px 10px 14px;
    background: var(--text-primary);
    color: var(--bg-deepest);
    border: 1.5px solid var(--text-primary);
    border-radius: 10px;
  }

  .repo-info.disabled {
    opacity: 0.6;
    pointer-events: none;
  }

  .repo-info :global(.repo-info-icon) {
    color: var(--bg-deepest);
  }

  .repo-details {
    min-width: 0;
    flex: 1;
  }

  .repo-name {
    font-size: var(--size-sm);
    color: var(--bg-deepest);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .recent-repos {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    margin-top: 4px;
  }

  .recent-chip {
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
    max-width: 100%;
    cursor: pointer;
    transition:
      background 0.15s,
      border-color 0.15s;
  }

  .recent-chip:hover {
    background: var(--chip-bg-hover);
    border-color: var(--chip-border-hover);
  }

  .chip-name {
    display: inline-flex;
    max-width: 250px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .chip-name :global(.repo-label-prefix) {
    color: inherit;
    opacity: 0.6;
  }

  .chip-name :global(.repo-label-emphasis) {
    color: inherit;
  }

  .recent-chip-shortcut {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    margin-left: 2px;
    padding: 1px 4px;
    background: var(--chip-shortcut-bg);
    border-radius: 3px;
    color: inherit;
    opacity: 0.5;
    font-size: 10px;
    flex-shrink: 0;
    line-height: 1;
  }
</style>
