<!--
  RepoDetailView.svelte - Detail view for a single repo.

  Shows the repo's default branch commit timeline when a local clone exists,
  or a download prompt when there is no local clone.
  Header displays repo short name + owner/repo (+ subpath) with badge color.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ArrowLeft, Download, GitCommitVertical, AlertTriangle, Plus } from 'lucide-svelte';
  import type { RepoHomeItem, CommitTimelineItem, RepoDefaultBranchTimeline } from '../../types';
  import * as commands from '../../api/commands';
  import { goHome } from '../layout/navigation.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import { badgeFg, badgeBg, badgeBgHover, badgeBorder } from '../../shared/badgeColors';
  import { formatRelativeTimeSeconds, minuteNow } from '../../shared/relativeTime.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import { alerts } from '../../shared/alerts.svelte';
  import TimelineRow from '../timeline/TimelineRow.svelte';
  import { listenToEvent } from '../../transport';

  interface Props {
    githubRepo: string;
    subpath: string;
  }

  let { githubRepo, subpath }: Props = $props();

  let repo = $state<RepoHomeItem | null>(null);
  let timeline = $state<RepoDefaultBranchTimeline | null>(null);
  let loading = $state(true);
  let timelineLoading = $state(false);
  let cloning = $state(false);
  let error = $state<string | null>(null);

  let accentColor = $derived(repo ? badgeFg(repo.hue, darkMode.value) : 'var(--text-primary)');
  let headerBgColor = $derived(repo ? badgeBg(repo.hue, darkMode.value) : 'transparent');
  let headerBgHoverColor = $derived(repo ? badgeBgHover(repo.hue, darkMode.value) : 'transparent');
  let headerBorderColor = $derived(
    repo ? badgeBorder(repo.hue, darkMode.value) : 'var(--border-subtle)'
  );

  let showDirtyPopover = $state(false);

  let subtitle = $derived.by(() => {
    if (subpath) return `${githubRepo}/${subpath}`;
    return githubRepo;
  });

  function handleDirtyClick(e: MouseEvent) {
    e.stopPropagation();
    showDirtyPopover = !showDirtyPopover;
  }

  function closeDirtyPopover() {
    showDirtyPopover = false;
  }

  function handleAddProjectFromChanges() {
    showDirtyPopover = false;
    window.dispatchEvent(
      new CustomEvent('staged:new-project', {
        detail: { githubRepo, subpath },
      })
    );
  }

  async function loadRepo() {
    loading = true;
    error = null;
    try {
      const all = await commands.listReposForHome();
      repo = all.find((r) => r.githubRepo === githubRepo && r.subpath === subpath) ?? null;

      if (repo?.hasLocalClone) {
        await loadTimeline();
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function loadTimeline() {
    timelineLoading = true;
    try {
      timeline = await commands.getRepoDefaultBranchTimeline(githubRepo, subpath, 50);
    } catch (e) {
      console.error('[RepoDetailView] Failed to load timeline:', e);
      error = e instanceof Error ? e.message : String(e);
    } finally {
      timelineLoading = false;
    }
  }

  async function handleClone() {
    if (cloning) return;
    cloning = true;
    try {
      await commands.cloneRepoLocally(githubRepo);
      alerts.show({
        tone: 'success',
        title: 'Clone complete',
        message: `${githubRepo} has been cloned locally.`,
        durationMs: 3000,
      });
      // Reload the repo data to get updated hasLocalClone state
      await loadRepo();
    } catch (e) {
      console.error('[RepoDetailView] Failed to clone repo:', e);
      alerts.show({
        tone: 'error',
        title: 'Clone failed',
        message: e instanceof Error ? e.message : String(e),
        durationMs: 5000,
      });
    } finally {
      cloning = false;
    }
  }

  onMount(() => {
    void loadRepo();

    let unlistenSync: (() => void) | undefined;
    listenToEvent<{ githubRepo: string; isDirty: boolean }>('repo-sync-update', (payload) => {
      if (payload.githubRepo === githubRepo && repo) {
        repo = { ...repo, isDirty: payload.isDirty };
      }
    }).then((fn) => (unlistenSync = fn));

    return () => {
      unlistenSync?.();
    };
  });
</script>

<div class="repo-detail-view">
  <!-- Header -->
  <div
    class="repo-header"
    style="--accent: {accentColor}; --header-bg: {headerBgColor}; --header-border: {headerBorderColor};"
  >
    <button class="back-btn" onclick={goHome} title="Back to home">
      <ArrowLeft size={16} />
    </button>
    <div class="header-info">
      <h1 class="repo-title">{repo?.shortName ?? githubRepo.split('/').pop()}</h1>
      <span class="repo-subtitle">{subtitle}</span>
      {#if timeline?.defaultBranch}
        <span class="default-branch-badge">{timeline.defaultBranch}</span>
      {/if}
      {#if repo?.isDirty && repo?.hasLocalClone}
        <div class="dirty-indicator-wrapper">
          <button
            class="dirty-indicator"
            title="Main branch has uncommitted changes"
            onclick={handleDirtyClick}
          >
            <AlertTriangle size={14} />
          </button>
          {#if showDirtyPopover}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="dirty-popover-backdrop"
              onclick={closeDirtyPopover}
              onkeydown={() => {}}
            ></div>
            <div class="dirty-popover">
              <p class="dirty-popover-message">
                Staged maintains the main branch. Uncommitted changes may be lost.
              </p>
              <button class="dirty-popover-action" onclick={handleAddProjectFromChanges}>
                <Plus size={12} />
                Add project from changes
              </button>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  </div>

  <!-- Content -->
  <div class="repo-content">
    {#if loading}
      <div class="state-container">
        <Spinner size={24} />
        <p class="state-text">Loading repo...</p>
      </div>
    {:else if error}
      <div class="state-container">
        <p class="state-text error">{error}</p>
        <button class="action-btn" onclick={() => loadRepo()}>Retry</button>
      </div>
    {:else if repo && !repo.hasLocalClone}
      <div class="state-container">
        <p class="no-clone-message">This repo has no local clone</p>
        <p class="no-clone-hint">
          Clone the repository locally to view commits, run actions, and open the code.
        </p>
        <button
          class="clone-btn"
          onclick={handleClone}
          disabled={cloning}
          style="--accent: {accentColor};"
        >
          {#if cloning}
            <Spinner size={16} />
            Cloning...
          {:else}
            <Download size={16} />
            Clone Repository
          {/if}
        </button>
      </div>
    {:else if repo}
      <!-- Timeline -->
      <div class="timeline-section">
        {#if timelineLoading}
          <div class="state-container compact">
            <Spinner size={18} />
          </div>
        {:else if timeline && timeline.commits.length > 0}
          <div class="timeline-list">
            {#each timeline.commits as commit (commit.sha)}
              <TimelineRow
                type="commit"
                title={commit.subject}
                meta={formatRelativeTimeSeconds(commit.timestamp, minuteNow.now())}
                secondaryMeta={commit.shortSha}
                tertiaryMeta={commit.isOwnCommit ? undefined : commit.author}
                isLast={commit === timeline.commits[timeline.commits.length - 1]}
              />
            {/each}
          </div>
        {:else}
          <div class="state-container compact">
            <p class="state-text">No commits on default branch</p>
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .repo-detail-view {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
  }

  .repo-header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 20px;
    background: var(--header-bg);
    border-bottom: 1px solid var(--header-border);
    flex-shrink: 0;
  }

  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    flex-shrink: 0;
    transition: all 0.15s ease;
  }

  .back-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
    border-color: var(--border-muted);
  }

  .header-info {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
    flex-wrap: wrap;
  }

  .repo-title {
    margin: 0;
    font-size: 20px;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: -0.02em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .repo-subtitle {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .default-branch-badge {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-tertiary);
    background: var(--bg-elevated);
    padding: 1px 6px;
    border-radius: 4px;
    white-space: nowrap;
  }

  .dirty-indicator-wrapper {
    position: relative;
    display: flex;
    align-items: center;
  }

  .dirty-indicator {
    display: flex;
    align-items: center;
    color: var(--ui-warning, #e5a100);
    background: none;
    border: none;
    padding: 3px;
    border-radius: 4px;
    cursor: pointer;
    transition: background 0.12s ease;
  }

  .dirty-indicator:hover {
    background: var(--bg-hover);
  }

  .dirty-popover-backdrop {
    position: fixed;
    inset: 0;
    z-index: 99;
  }

  .dirty-popover {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 100;
    width: 240px;
    padding: 10px 12px;
    margin-top: 4px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .dirty-popover-message {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-secondary);
    line-height: 1.4;
  }

  .dirty-popover-action {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    font-size: var(--size-xs);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.12s ease;
    white-space: nowrap;
  }

  .dirty-popover-action:hover {
    background: var(--bg-hover);
    border-color: var(--border-muted);
  }

  .repo-content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0;
  }

  .state-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 60px 20px;
    text-align: center;
  }

  .state-container.compact {
    padding: 24px 20px;
  }

  .state-text {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
  }

  .state-text.error {
    color: var(--ui-danger);
  }

  .no-clone-message {
    margin: 0;
    font-size: var(--size-md);
    font-weight: 600;
    color: var(--text-secondary);
  }

  .no-clone-hint {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--text-muted);
    max-width: 360px;
    line-height: 1.5;
  }

  .clone-btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 10px 20px;
    border: none;
    border-radius: 8px;
    background-color: var(--accent);
    color: var(--bg-deepest);
    font-size: var(--size-sm);
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s ease;
    margin-top: 4px;
  }

  .clone-btn:hover:not(:disabled) {
    opacity: 0.85;
  }

  .clone-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .action-btn {
    padding: 7px 16px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: transparent;
    color: var(--text-secondary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .action-btn:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
  }

  .timeline-section {
    padding: 0;
  }

  .timeline-list {
    display: flex;
    flex-direction: column;
    padding: 8px 16px;
  }
</style>
