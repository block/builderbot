<!--
  ProjectsList.svelte - Landing page listing all projects

  Clicking a project navigates to its project page.
-->
<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { fade } from 'svelte/transition';
  import Cloud from '@lucide/svelte/icons/cloud';
  import GitPullRequest from '@lucide/svelte/icons/git-pull-request';
  import GitPullRequestClosed from '@lucide/svelte/icons/git-pull-request-closed';
  import GitPullRequestDraft from '@lucide/svelte/icons/git-pull-request-draft';
  import Mail from '@lucide/svelte/icons/mail';
  import Plus from '@lucide/svelte/icons/plus';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import Sprout from '@lucide/svelte/icons/sprout';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import type { Project, WorkspaceStatus } from '../../types';
  import RepoCard from './RepoCard.svelte';
  import {
    projectDisplayName,
    aggregateProjectPrStatus,
    projectHasCodeChanges,
    projectActivity,
  } from '../../shared/utils';
  import { projectStateStore } from '../../stores/projectState.svelte';
  import { projectRunActionsStore } from '../../stores/projectRunActions.svelte';
  import { projectsDataStore } from '../../stores/projectsData.svelte';
  import { openSettings, selectProject, showAllRepos } from '../layout/navigation.svelte';
  import NewProjectModal from './NewProjectModal.svelte';
  import { getProjectStatus } from './projectStatus';
  import { projectActions } from './projectActions.svelte';
  import * as ContextMenu from '$lib/components/ui/context-menu';
  import SplashScreen from './SplashScreen.svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import SineWave from '../../shared/SineWave.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import { Button } from '$lib/components/ui/button';

  import {
    finishProjectsListRestore,
    projectsListViewState,
    setProjectsListScrollTop,
  } from './projectsListViewState.svelte';
  import { darkMode } from '../../stores/isDark.svelte';
  import { repoBadgeStore } from '../../stores/repoBadges.svelte';
  import { badgeBg, badgeFg, badgeBgHover } from '../../shared/badgeColors';
  import { repoSeedFromNewProjectEvent } from './newProjectEvent';
  import type { RepoSelection } from '../../shared/githubUrl';
  import { viewport } from '../../shared/viewport.svelte';
  import TopBarPortal from '../layout/TopBarPortal.svelte';

  type FilterKind = 'unread' | 'running' | { repo: string; subpath: string };

  // Data comes from the shared projectsData store — returning to the landing
  // page paints instantly from memory while the store revalidates in the
  // background. Filters, scroll restore, and modal state stay view-local.
  let projects = $derived(projectsDataStore.projects);
  let projectBranches = $derived(projectsDataStore.branchesByProject);
  let reposByProject = $derived(projectsDataStore.reposByProject);
  let deletingProjectNames = $derived(projectsDataStore.deletingProjectNames);
  let homeRepos = $derived(projectsDataStore.homeRepos);
  // The grid paints every card complete or not at all, so it waits for each
  // project's branches and repos — not just the project list `loaded` covers.
  let loading = $derived(
    projectsDataStore.loading || !projectsDataStore.loaded || !projectsDataStore.allProjectsHydrated
  );
  let error = $derived(projectsDataStore.error);

  let showNewProjectModal = $state(false);
  let newProjectInitialRepo = $state<RepoSelection | null>(null);
  let isCommandKeyHeld = $state(false);
  let mainPanelEl = $state<HTMLDivElement | null>(null);
  let activeFilters = $state<Set<string>>(new Set());
  let restoreInProgress = false;
  let restoreToken = 0;
  const projectCardElements = new Map<string, HTMLElement>();

  /** Unique repo+subpath entries sorted alphabetically by full display string */
  let repoFilters = $derived.by(() => {
    const counts = new Map<string, { repo: string; subpath: string; count: number }>();
    for (const project of projects) {
      const repos = reposByProject.get(project.id) ?? [];
      if (repos.length > 0) {
        for (const r of repos) {
          const displayRepo = r.headRepo ?? r.githubRepo;
          const key = `${displayRepo}:${r.subpath ?? ''}`;
          const entry = counts.get(key);
          if (entry) {
            entry.count++;
          } else {
            counts.set(key, { repo: displayRepo, subpath: r.subpath ?? '', count: 1 });
          }
        }
      } else if (project.githubRepo) {
        const key = `${project.githubRepo}:${project.subpath ?? ''}`;
        const entry = counts.get(key);
        if (entry) {
          entry.count++;
        } else {
          counts.set(key, { repo: project.githubRepo, subpath: project.subpath ?? '', count: 1 });
        }
      }
    }
    return [...counts.values()].sort((a, b) => {
      const aDisplay = a.subpath ? `${a.repo}/${a.subpath}` : a.repo;
      const bDisplay = b.subpath ? `${b.repo}/${b.subpath}` : b.repo;
      return aDisplay.localeCompare(bDisplay);
    });
  });

  function filterKey(filter: FilterKind): string {
    if (typeof filter === 'string') return filter;
    return `repo:${filter.repo}:${filter.subpath}`;
  }

  let allFilters = $derived.by(() => {
    const filters: FilterKind[] = ['unread', 'running'];
    for (const rf of repoFilters) {
      filters.push({ repo: rf.repo, subpath: rf.subpath });
    }
    return filters;
  });

  let unreadCount = $derived(projects.filter((p) => projectStateStore.isUnread(p.id)).length);

  let runningCount = $derived(
    projects.filter((p) => {
      const status = getProjectStatus(p.id, deletingProjectNames, projectBranches.get(p.id) || []);
      return status.kind === 'running' || status.kind === 'runAction';
    }).length
  );

  let hasRepoFilters = $derived(
    [...activeFilters].some((key) => key !== 'unread' && key !== 'running')
  );

  let filteredProjects = $derived.by(() => {
    if (activeFilters.size === 0) return projects;
    return projects.filter((p) => {
      // Status filters are AND'd with each other and with repo filters
      if (activeFilters.has('unread') && !projectStateStore.isUnread(p.id)) return false;
      if (activeFilters.has('running')) {
        const status = getProjectStatus(
          p.id,
          deletingProjectNames,
          projectBranches.get(p.id) || []
        );
        if (status.kind !== 'running' && status.kind !== 'runAction') return false;
      }
      // Repo filters are OR'd with each other
      if (!hasRepoFilters) return true;
      const repos = reposByProject.get(p.id) ?? [];
      if (repos.length > 0) {
        return repos.some((r) =>
          activeFilters.has(
            filterKey({ repo: r.headRepo ?? r.githubRepo, subpath: r.subpath ?? '' })
          )
        );
      }
      if (p.githubRepo) {
        return activeFilters.has(filterKey({ repo: p.githubRepo, subpath: p.subpath ?? '' }));
      }
      return false;
    });
  });

  function toggleFilter(filter: FilterKind, event?: MouseEvent) {
    const key = filterKey(filter);

    if (event?.shiftKey) {
      // Shift+click: toggle individual filter
      const next = new Set(activeFilters);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      activeFilters = next;
    } else {
      // Plain click: switch to this filter exclusively
      if (activeFilters.size === 1 && activeFilters.has(key)) {
        // Clicking the only active filter deselects it (back to showing all)
        activeFilters = new Set();
      } else {
        activeFilters = new Set([key]);
      }
    }
  }

  function isFilterActive(filter: FilterKind): boolean {
    return activeFilters.has(filterKey(filter));
  }

  function trackProjectCard(node: HTMLElement, projectId: string) {
    let currentProjectId = projectId;
    projectCardElements.set(currentProjectId, node);

    return {
      update(nextProjectId: string) {
        if (nextProjectId === currentProjectId) return;
        if (projectCardElements.get(currentProjectId) === node) {
          projectCardElements.delete(currentProjectId);
        }
        currentProjectId = nextProjectId;
        projectCardElements.set(currentProjectId, node);
      },
      destroy() {
        if (projectCardElements.get(currentProjectId) === node) {
          projectCardElements.delete(currentProjectId);
        }
      },
    };
  }

  function handleMainPanelScroll() {
    if (!mainPanelEl) return;
    setProjectsListScrollTop(mainPanelEl.scrollTop);
  }

  async function restoreProjectsListPosition() {
    if (!mainPanelEl || restoreInProgress) return;

    restoreInProgress = true;
    const token = ++restoreToken;
    let restored = false;

    try {
      await tick();
      if (token !== restoreToken || !mainPanelEl) return;

      mainPanelEl.scrollTop = projectsListViewState.scrollTop;
      const targetProjectId = projectsListViewState.returnTargetProjectId;
      const targetEl = targetProjectId ? projectCardElements.get(targetProjectId) : null;
      targetEl?.scrollIntoView({ block: 'nearest' });
      restored = true;
    } finally {
      if (token === restoreToken) {
        restoreInProgress = false;
        if (restored) {
          finishProjectsListRestore();
        }
      }
    }
  }

  $effect(() => {
    const readyToRestore =
      projectsListViewState.restorePending &&
      !restoreInProgress &&
      !loading &&
      !error &&
      filteredProjects.length > 0 &&
      mainPanelEl;

    if (!readyToRestore) return;
    void restoreProjectsListPosition();
  });

  onMount(() => {
    // Backend/window listeners for the shared data live in the projectsData
    // store, started once from App.svelte.
    void projectsDataStore.ensureLoaded();
    void projectsDataStore.ensureHomeReposLoaded();
    void projectRunActionsStore.startListening();

    const onNewProject = (event: Event) => {
      newProjectInitialRepo = repoSeedFromNewProjectEvent(event);
      showNewProjectModal = true;
    };
    window.addEventListener('staged:new-project', onNewProject);

    return () => {
      projectRunActionsStore.stopListening();
      window.removeEventListener('staged:new-project', onNewProject);
    };
  });

  // Pull every project's branches and repos forward off the store's idle drip
  // — the grid renders all of them. An effect rather than onMount so a list
  // change or a cache-stale reload re-kicks the sweep and the loading gate
  // heals itself; the store dedupes projects it has already fetched.
  $effect(() => {
    if (!projectsDataStore.loaded) return;
    void projectsDataStore.ensureProjectsHydrated();
  });

  // Keep run-action state hydrated for the status badges; the store call
  // dedupes branches it has already queried.
  $effect(() => {
    projectRunActionsStore
      .hydrateFromProjectBranches(projectsDataStore.branchesByProject)
      .catch(console.error);
  });

  function handleProjectCreated(project: Project) {
    projectsDataStore.projectCreated(project);
    showNewProjectModal = false;
    selectProject(project.id);
  }

  function isProjectDeleting(projectId: string): boolean {
    return projectsDataStore.isProjectDeleting(projectId);
  }

  function openProject(projectId: string) {
    if (isProjectDeleting(projectId)) return;
    if (mainPanelEl) {
      setProjectsListScrollTop(mainPanelEl.scrollTop);
    }
    selectProject(projectId);
  }

  function getProjectPrStatus(
    projectId: string
  ): 'merged' | 'open' | 'closed' | 'checks_failing' | 'conflict' | null {
    const branches = projectBranches.get(projectId) || [];
    return aggregateProjectPrStatus(branches);
  }

  function getProjectWorkspaceStatus(projectId: string): WorkspaceStatus | null {
    const branches = projectBranches.get(projectId) || [];
    return branches.find((b) => b.workspaceStatus)?.workspaceStatus ?? null;
  }

  function cloudStatusClass(status: WorkspaceStatus | null): string {
    switch (status) {
      case 'running':
        return 'cloud-running';
      case 'starting':
        return 'cloud-starting';
      case 'error':
        return 'cloud-error';
      case 'stopped':
      case 'suspended':
      default:
        return 'cloud-inactive';
    }
  }

  function verifyCommandKeyState(e: KeyboardEvent | MouseEvent) {
    // Verify the command key is actually held down by checking the event's metaKey/ctrlKey
    const actuallyHeld = e.metaKey || e.ctrlKey;
    if (isCommandKeyHeld && !actuallyHeld) {
      isCommandKeyHeld = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    const target = e.target as HTMLElement;
    const isInput = target.tagName === 'INPUT' || target.tagName === 'TEXTAREA';
    if (isInput) return;

    // Track command key state
    if (e.metaKey || e.ctrlKey) {
      isCommandKeyHeld = true;
    } else {
      // Any non-command key press while we think command is held means it's not
      verifyCommandKeyState(e);
    }

    // Command+1-9 to open projects by number
    if ((e.metaKey || e.ctrlKey) && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      const index = parseInt(e.key) - 1;
      if (index < projects.length) {
        openProject(projects[index].id);
      }
    }
  }

  function handleKeyup(e: KeyboardEvent) {
    // Verify the actual state first
    verifyCommandKeyState(e);
    if (!e.metaKey && !e.ctrlKey) {
      isCommandKeyHeld = false;
    }
  }

  function handleBlur() {
    // Reset command key state when window loses focus
    // This handles cases like Command+Tab where keyup isn't received
    isCommandKeyHeld = false;
  }

  function handleMouseMove(e: MouseEvent) {
    // Verify command key state on mouse movement
    // This catches cases where the state got stuck (e.g., Command+Tab back to app)
    if (isCommandKeyHeld) {
      verifyCommandKeyState(e);
    }
  }
</script>

<svelte:window
  onkeydown={handleKeydown}
  onkeyup={handleKeyup}
  onblur={handleBlur}
  onmousemove={handleMouseMove}
/>

<TopBarPortal title="Projects" rightActions={rootTopBarActions} />

{#snippet rootTopBarActions()}
  <span class="inline-flex" title={viewport.showShortcutHints ? 'New project (⌘N)' : 'New project'}>
    <Button
      variant="ghost"
      size="icon-xs"
      class="max-md:size-10 [&_svg]:size-3.5"
      aria-label="New project"
      onclick={() => {
        newProjectInitialRepo = null;
        showNewProjectModal = true;
      }}
    >
      <Plus size={14} />
    </Button>
  </span>

  <Button
    variant="ghost"
    size="icon-xs"
    class="max-md:size-10 [&_svg]:size-3.5"
    title={viewport.showShortcutHints ? 'Settings (⌘,)' : 'Settings'}
    aria-label="Settings"
    onclick={() => openSettings()}
  >
    <SlidersHorizontal size={14} />
  </Button>
{/snippet}

<div class="projects-list-page">
  <div class="main-panel" bind:this={mainPanelEl} onscroll={handleMainPanelScroll}>
    <div class="content" class:empty-layout={!loading && !error && projects.length === 0}>
      {#if error}
        <div class="state error">{error}</div>
      {:else if loading}
        <div class="state">Loading projects…</div>
      {:else if projects.length === 0}
        <SplashScreen
          onCreated={handleProjectCreated}
          requestOpen={showNewProjectModal && projects.length === 0}
          onFormOpenChange={(open) => (showNewProjectModal = open)}
        />
      {:else}
        {#if homeRepos.length > 0}
          <div class="repos-section">
            <div class="repos-header">
              <h2 class="repos-title">Repos</h2>
              <Button
                variant="ghost"
                size="sm"
                class="h-7 text-muted-foreground"
                onclick={showAllRepos}>View all</Button
              >
            </div>
            <div class="repos-scroll-row">
              {#each homeRepos as repo (repo.githubRepo + ':' + repo.subpath)}
                <RepoCard {repo} onChange={() => projectsDataStore.refreshHomeRepos()} />
              {/each}
            </div>
          </div>
        {/if}

        {#if projects.length > 0}
          <div class="filter-bar">
            <button
              class="filter-chip"
              class:active={isFilterActive('unread')}
              onclick={(e: MouseEvent) => toggleFilter('unread', e)}
              disabled={unreadCount === 0 && !isFilterActive('unread')}
            >
              Unread
              <span class="filter-count">{unreadCount}</span>
            </button>
            <button
              class="filter-chip"
              class:active={isFilterActive('running')}
              onclick={(e: MouseEvent) => toggleFilter('running', e)}
              disabled={runningCount === 0 && !isFilterActive('running')}
            >
              Running
              <span class="filter-count">{runningCount}</span>
            </button>
            {#each repoFilters as rf}
              {@const badge = repoBadgeStore.lookup(rf.repo, rf.subpath || undefined)}
              {@const filter = { repo: rf.repo, subpath: rf.subpath }}
              {@const active = isFilterActive(filter)}
              <button
                class="filter-chip repo-filter"
                class:active
                onclick={(e: MouseEvent) => toggleFilter(filter, e)}
                style={badge
                  ? `--repo-bg: ${badgeBg(badge.hue, darkMode.value)}; --repo-fg: ${badgeFg(badge.hue, darkMode.value)}; --repo-bg-hover: ${badgeBgHover(badge.hue, darkMode.value)}`
                  : ''}
              >
                <RepoLabel githubRepo={rf.repo} subpath={rf.subpath || null} />
                <span class="filter-count">{rf.count}</span>
              </button>
            {/each}
          </div>
        {/if}
        <div class="projects-grid">
          {#each filteredProjects as project, index (project.id)}
            {@const status = getProjectStatus(
              project.id,
              deletingProjectNames,
              projectBranches.get(project.id) || []
            )}
            {@const prStatus = getProjectPrStatus(project.id)}
            {@const repos = reposByProject.get(project.id) ?? []}
            {@const sessionTypes = projectStateStore.getRunningSessionTypes(project.id)}
            {@const activity = projectActivity(sessionTypes, status.runActionPhase)}
            {@const workspaceStatus =
              project.location === 'remote' ? getProjectWorkspaceStatus(project.id) : null}
            <div class="project-card-wrapper" use:trackProjectCard={project.id}>
              <ContextMenu.Root>
                <ContextMenu.Trigger class="contents" disabled={status.kind === 'deleting'}>
                  <button
                    class="project-card"
                    class:deleting={status.kind === 'deleting'}
                    onclick={() => openProject(project.id)}
                    disabled={status.kind === 'deleting'}
                    title={status.kind === 'deleting' ? 'Project deletion in progress' : undefined}
                  >
                    {#if viewport.showShortcutHints && isCommandKeyHeld && index < 9}
                      <div class="keyboard-shortcut-overlay">
                        <span class="command-icon">⌘</span>
                        <span class="number">{index + 1}</span>
                      </div>
                    {/if}
                    {#if status.runActionPhase === 'running' && status.kind === 'running'}
                      <div
                        class="status-indicator wave-spinner"
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      >
                        <SineWave size={14} />
                        <Spinner size={14} />
                      </div>
                    {:else if status.runActionPhase === 'running'}
                      <div
                        class="status-indicator wave"
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      >
                        <SineWave size={14} />
                      </div>
                    {:else if status.kind === 'runAction' || status.kind === 'running'}
                      <div
                        class="status-indicator spinner"
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      >
                        <Spinner size={14} />
                      </div>
                    {:else if status.kind === 'unread'}
                      <div
                        class="status-indicator unread-dot"
                        in:fade={{ duration: 300, delay: 150 }}
                        out:fade={{ duration: 150 }}
                      ></div>
                    {/if}
                    <div class="card-header">
                      {#if project.location === 'remote'}
                        <Cloud size={16} class={cloudStatusClass(workspaceStatus)} />
                      {:else if prStatus === 'merged'}
                        <GitPullRequest size={16} class="pr-status-merged" />
                      {:else if prStatus === 'checks_failing'}
                        <GitPullRequest size={16} class="pr-status-checks-failing" />
                      {:else if prStatus === 'open'}
                        <GitPullRequest size={16} />
                      {:else if prStatus === 'closed'}
                        <GitPullRequestClosed size={16} />
                      {:else if prStatus === 'conflict'}
                        <GitPullRequestClosed size={16} class="pr-status-conflict" />
                      {:else if projectHasCodeChanges(projectBranches.get(project.id) || [])}
                        <GitPullRequestDraft size={16} class="pr-status-draft" />
                      {:else}
                        <Sprout size={16} class="pr-status-clean" />
                      {/if}
                      <span>{projectDisplayName(project)}</span>
                    </div>
                    {#if status.kind === 'deleting'}
                      <div class="deleting-pill" role="status" aria-live="polite">Deleting…</div>
                    {/if}
                    <div class="repo">
                      {#if repos.length > 0}
                        {#each [...repos].sort((a, b) => {
                          const aKey = a.subpath ? `${a.githubRepo}/${a.subpath}` : a.githubRepo;
                          const bKey = b.subpath ? `${b.githubRepo}/${b.subpath}` : b.githubRepo;
                          return aKey.localeCompare(bKey);
                        }) as r}
                          {@const badge = repoBadgeStore.lookup(r.githubRepo, r.subpath)}
                          {#if badge}
                            <span
                              class="repo-badge-label"
                              style="background: {badgeBg(
                                badge.hue,
                                darkMode.value
                              )}; color: {badgeFg(badge.hue, darkMode.value)};"
                            >
                              <RepoLabel
                                githubRepo={r.headRepo ?? r.githubRepo}
                                subpath={r.subpath}
                              />
                            </span>
                          {:else}
                            <span class="repo-line">
                              <RepoLabel
                                githubRepo={r.headRepo ?? r.githubRepo}
                                subpath={r.subpath}
                              />
                            </span>
                          {/if}
                        {/each}
                      {:else if project.githubRepo}
                        {@const badge = repoBadgeStore.lookup(project.githubRepo, project.subpath)}
                        {#if badge}
                          <span
                            class="repo-badge-label"
                            style="background: {badgeBg(
                              badge.hue,
                              darkMode.value
                            )}; color: {badgeFg(badge.hue, darkMode.value)};"
                          >
                            <RepoLabel githubRepo={project.githubRepo} subpath={project.subpath} />
                          </span>
                        {:else}
                          <span class="repo-line">
                            <RepoLabel githubRepo={project.githubRepo} subpath={project.subpath} />
                          </span>
                        {/if}
                      {:else}
                        No repo attached
                      {/if}
                    </div>
                  </button>
                </ContextMenu.Trigger>
                <ContextMenu.Content class="min-w-[172px]">
                  <ContextMenu.Item
                    disabled={status.kind === 'deleting'}
                    onSelect={() => projectActions.markProjectUnread(project)}
                  >
                    <Mail size={14} /> Mark as Unread
                  </ContextMenu.Item>
                  <ContextMenu.Item
                    variant="destructive"
                    disabled={status.kind === 'deleting'}
                    onSelect={() => projectActions.requestRemoveProject(project)}
                  >
                    <Trash2 size={14} /> Remove Project
                  </ContextMenu.Item>
                </ContextMenu.Content>
              </ContextMenu.Root>
              {#if activity}
                <div class="card-location">
                  {activity}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

<NewProjectModal
  open={showNewProjectModal && projects.length > 0}
  initialRepo={newProjectInitialRepo}
  onCreated={handleProjectCreated}
  onClose={() => (showNewProjectModal = false)}
/>

<style>
  .projects-list-page {
    flex: 1;
    min-height: 0;
    display: flex;
    min-width: 0;
    background-color: var(--bg-chrome);
    overflow: hidden;
  }

  .main-panel {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow-y: scroll;
  }

  .content {
    flex: 1;
    padding: 24px;
    max-width: 900px;
    width: 100%;
    margin: 0 auto;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    min-height: 100%;
  }

  .content.empty-layout {
    max-width: none;
    padding: 0;
  }

  .state {
    color: var(--text-muted);
    padding: 16px 2px;
  }

  .state.error {
    color: var(--ui-danger);
  }

  .filter-bar {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }

  .filter-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    background: var(--bg-elevated);
    color: var(--text-secondary);
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
    white-space: nowrap;
  }

  .filter-chip:hover:not(:disabled) {
    background: var(--bg-hover);
    border-color: var(--border-emphasis);
  }

  .filter-chip.active:hover:not(:disabled) {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
  }

  .filter-chip:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .filter-chip.active {
    background: var(--ui-accent);
    border-color: var(--ui-accent);
    color: white;
  }

  .filter-chip.repo-filter {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    font-size: 11px;
    font-weight: 600;
    background: var(--repo-bg, var(--bg-elevated));
    color: var(--repo-fg, var(--text-secondary));
    border-color: transparent;
  }

  .filter-chip.repo-filter:hover:not(:disabled) {
    background: var(--repo-bg-hover, var(--bg-hover));
  }

  .filter-chip.repo-filter.active {
    box-shadow: 0 0 0 2px var(--repo-fg, var(--ui-accent));
    background: var(--repo-bg, var(--ui-accent));
    color: var(--repo-fg, white);
    border-color: transparent;
  }

  .filter-count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: 999px;
    background: rgba(128, 128, 128, 0.15);
    font-size: 11px;
    font-weight: 600;
    line-height: 1;
  }

  .filter-chip.active .filter-count {
    background: rgba(255, 255, 255, 0.25);
  }

  .filter-chip.repo-filter .filter-count {
    background: rgba(128, 128, 128, 0.15);
  }

  .filter-chip.repo-filter.active .filter-count {
    background: rgba(128, 128, 128, 0.2);
  }

  .filter-chip.repo-filter :global(.repo-label-prefix) {
    color: inherit;
    opacity: 0.6;
  }

  .filter-chip.repo-filter :global(.repo-label-emphasis) {
    color: inherit;
  }

  .repos-section {
    margin-bottom: 24px;
  }

  .repos-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .repos-title {
    margin: 0;
    font-size: var(--size-lg);
    font-weight: 700;
    color: var(--text-primary);
  }

  .repos-scroll-row {
    display: flex;
    gap: 10px;
    overflow-x: auto;
    padding-bottom: 4px;
    scrollbar-width: thin;
    scrollbar-color: var(--border-muted) transparent;
  }

  .repos-scroll-row > :global(.repo-card) {
    width: 200px;
    flex-shrink: 0;
  }

  .repos-scroll-row::-webkit-scrollbar {
    height: 4px;
  }

  .repos-scroll-row::-webkit-scrollbar-track {
    background: transparent;
  }

  .repos-scroll-row::-webkit-scrollbar-thumb {
    background: var(--border-muted);
    border-radius: 2px;
  }

  .projects-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    grid-auto-rows: 1fr;
    gap: 12px;
  }

  .project-card-wrapper {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .project-card-wrapper .project-card {
    flex: 1;
  }

  .card-location {
    color: var(--text-faint);
    font-size: var(--size-xs);
    padding: 0 4px;
  }

  .project-card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
    text-align: left;
    background: var(--bg-primary);
    border: none;
    border-radius: 10px;
    padding: 16px;
    min-height: 120px;
    color: inherit;
    cursor: pointer;
    transition: background-color 0.15s ease;
  }

  .project-card:hover {
    background-color: var(--bg-hover);
  }

  .project-card:disabled {
    cursor: not-allowed;
  }

  .project-card.deleting {
    opacity: 0.75;
  }

  .project-card.deleting:hover {
    background: var(--bg-primary);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-primary);
    font-size: var(--size-lg);
    font-weight: 600;
    padding-right: 24px;
  }

  .card-header :global(svg) {
    flex-shrink: 0;
  }

  .card-header :global(svg.pr-status-merged) {
    stroke: var(--ui-success);
  }

  .card-header :global(svg.pr-status-conflict) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.pr-status-checks-failing) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.pr-status-draft) {
    stroke: var(--text-muted);
  }

  .card-header :global(svg.pr-status-clean) {
    stroke: var(--text-faint);
  }

  .card-header :global(svg.cloud-running) {
    stroke: var(--ui-accent);
  }

  .card-header :global(svg.cloud-starting) {
    stroke: var(--ui-info);
  }

  .card-header :global(svg.cloud-error) {
    stroke: var(--ui-danger);
  }

  .card-header :global(svg.cloud-inactive) {
    stroke: var(--text-muted);
  }

  .repo {
    margin-top: auto;
    font-size: var(--size-sm);
    display: flex;
    flex-direction: column;
    gap: 2px;
    overflow: hidden;
  }

  .repo-line {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .repo-badge-label {
    display: inline-flex;
    align-items: center;
    padding: 1px 5px;
    border-radius: 4px;
    font-weight: 600;
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .repo-badge-label :global(.repo-label-prefix) {
    color: inherit;
    opacity: 0.6;
  }

  .repo-badge-label :global(.repo-label-emphasis) {
    color: inherit;
  }

  .deleting-pill {
    width: fit-content;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--border-muted);
    background-color: var(--bg-elevated);
    color: var(--text-primary);
    font-size: calc(var(--size-xs) - 1px);
    font-weight: 600;
  }

  .keyboard-shortcut-overlay {
    position: absolute;
    top: 8px;
    right: 8px;
    display: flex;
    align-items: center;
    gap: 4px;
    background-color: var(--bg-elevated);
    border: 1px solid var(--border-emphasis);
    border-radius: 6px;
    padding: 4px 8px;
    font-size: var(--size-xs);
    font-weight: 600;
    color: var(--text-primary);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
    z-index: 10;
    pointer-events: none;
  }

  .keyboard-shortcut-overlay .command-icon {
    color: var(--ui-accent);
    font-size: var(--size-sm);
  }

  .keyboard-shortcut-overlay .number {
    font-family: 'SF Mono', 'Menlo', 'Consolas', monospace;
    color: var(--ui-accent);
  }

  .status-indicator {
    position: absolute;
    top: 10px;
    right: 10px;
    z-index: 5;
  }

  .status-indicator.spinner {
    color: var(--ui-accent);
  }

  .status-indicator.wave {
    color: var(--ui-accent);
  }

  .status-indicator.wave-spinner {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--ui-accent);
  }

  .status-indicator.unread-dot {
    width: 8px;
    height: 8px;
    background-color: var(--ui-accent);
    border-radius: 50%;
  }

  @media (max-width: 900px) {
    .projects-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }

  @media (max-width: 640px) {
    .content {
      padding: 16px;
    }

    .filter-bar {
      flex-wrap: nowrap;
      gap: 8px;
      overflow-x: auto;
      margin: 0 -16px 14px;
      padding: 0 16px 4px;
    }

    .filter-chip {
      min-height: 36px;
    }

    .projects-grid {
      grid-template-columns: minmax(0, 1fr);
      gap: 10px;
    }

    .project-card {
      min-height: 104px;
      padding: 14px;
    }

    .card-header {
      font-size: var(--size-md);
    }
  }
</style>
