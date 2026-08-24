/**
 * Typed invoke wrappers for Tauri commands.
 *
 * One function per command. Each returns a typed promise.
 */

import { invokeCommand, isTauri } from './transport';
import {
  cachedCommand,
  cachedInvoke,
  invalidateCacheByCommand,
  invalidateCache,
  type SwrResult,
} from './cache';
import { readSnapshot, writeSnapshot, SNAPSHOT_KEYS } from './shared/webSnapshot';
import type {
  Project,
  ProjectRepo,
  RecentRepo,
  GitHubRepo,
  Branch,
  BranchTimeline,
  BranchRef,
  BranchPipelineResponse,
  BranchSessionLaunchContext,
  BranchSessionType,
  BranchSessionResponse,
  StoreIncompatibility,
  ActiveSessionInfo,
  Session,
  SessionMessage,
  QueuedSessionMessage,
  DiffFilesResponse,
  FileDiff,
  File,
  Review,
  Comment,
  BranchNote,
  WorkspaceInfo,
  PullRequest,
  Issue,
  PrStatus,
  PollWorkspaceResult,
  Image,
  SuggestedRepo,
  AcpConfigSelection,
} from './types';

export type DiffScope = 'branch' | 'commit' | 'worktree';

export interface WorktreeChangesPreview {
  revertPaths: string[];
  removePaths: string[];
  conflictedPaths: string[];
}

// =============================================================================
// Store status
// =============================================================================

/** Returns null if the store is ready, or version info if a reset is needed. */
export function getStoreStatus(): Promise<StoreIncompatibility | null> {
  return invokeCommand('get_store_status');
}

/** Delete the old database and create a fresh store. Called after user confirms. */
export function confirmResetStore(): Promise<void> {
  return invokeCommand('confirm_reset_store');
}

// =============================================================================
// Projects
// =============================================================================

export interface ForceRefreshOptions {
  force?: boolean;
}

export function listProjects(options: ForceRefreshOptions = {}): Promise<SwrResult<Project[]>> {
  return cachedCommand('list_projects', undefined, {
    ttl: 5 * 60_000,
    bypassRead: options.force,
  });
}

export async function createProject(
  name: string,
  location: 'local' | 'remote',
  githubRepo?: string,
  subpath?: string,
  branchName?: string,
  prNumber?: number,
  defaultBranch?: string,
  headRepo?: string
): Promise<Project> {
  const project = await invokeCommand<Project>('create_project', {
    name,
    location,
    githubRepo: githubRepo ?? null,
    subpath,
    branchName: branchName ?? null,
    prNumber: prNumber ?? null,
    defaultBranch: defaultBranch ?? null,
    headRepo: headRepo ?? null,
  });
  await invalidateCacheByCommand('list_projects');
  return project;
}

export async function deleteProject(id: string): Promise<void> {
  await invokeCommand('delete_project', { id });
  await Promise.all([
    invalidateCacheByCommand('list_projects'),
    invalidateCacheByCommand('list_branches_for_project'),
    invalidateCacheByCommand('list_project_repos'),
  ]);
}

export function listProjectRepos(
  projectId: string,
  options: ForceRefreshOptions = {}
): Promise<SwrResult<ProjectRepo[]>> {
  return cachedCommand(
    'list_project_repos',
    { projectId },
    {
      ttl: 10 * 60_000,
      bypassRead: options.force,
    }
  );
}

export function listRecentRepos(limit?: number): Promise<RecentRepo[]> {
  return invokeCommand('list_recent_repos', { limit: limit ?? 10 });
}

export async function addProjectRepo(
  projectId: string,
  githubRepo: string,
  branchName?: string,
  subpath?: string,
  setAsPrimary?: boolean,
  prNumber?: number,
  defaultBranch?: string,
  headRepo?: string
): Promise<ProjectRepo> {
  const repo = await invokeCommand<ProjectRepo>('add_project_repo', {
    projectId,
    githubRepo,
    branchName: branchName ?? null,
    subpath: subpath ?? null,
    setAsPrimary: setAsPrimary ?? null,
    prNumber: prNumber ?? null,
    defaultBranch: defaultBranch ?? null,
    headRepo: headRepo ?? null,
  });
  await invalidateCache('list_project_repos', { projectId });
  return repo;
}

export function updateProjectRepoBranchName(
  projectId: string,
  projectRepoId: string,
  branchName: string
): Promise<void> {
  return invokeCommand('update_project_repo_branch_name', { projectId, projectRepoId, branchName });
}

export async function removeProjectRepo(projectId: string, projectRepoId: string): Promise<void> {
  await invokeCommand('remove_project_repo', { projectId, projectRepoId });
  await invalidateCache('list_project_repos', { projectId });
}

export async function setPrimaryProjectRepo(
  projectId: string,
  projectRepoId: string
): Promise<void> {
  await invokeCommand('set_primary_project_repo', { projectId, projectRepoId });
  await invalidateCache('list_project_repos', { projectId });
}

export function getSuggestedRepos(projectId: string, limit?: number): Promise<SuggestedRepo[]> {
  return invokeCommand('get_suggested_repos', { projectId, limit: limit ?? null });
}

/** List the authenticated user's GitHub organization memberships. */
export function listGithubOrgs(): Promise<string[]> {
  return invokeCommand('list_github_orgs');
}

/** List GitHub repositories for the authenticated user or a specific owner. */
export function listGithubRepos(owner?: string): Promise<GitHubRepo[]> {
  return invokeCommand('list_github_repos', { owner: owner ?? null });
}

/** List repositories the authenticated user has recently pushed to.
 *  Returns repos across all orgs, sorted by most recently pushed. */
export function listUserRepos(limit?: number): Promise<GitHubRepo[]> {
  return invokeCommand('list_user_repos', { limit: limit ?? null });
}

/** Fetch a single GitHub repository by owner/repo.
 *  Returns null if the repo doesn't exist or user lacks access. */
export function getGithubRepo(owner: string, repo: string): Promise<GitHubRepo | null> {
  return invokeCommand('get_github_repo', { owner, repo });
}

/** Search GitHub repositories for the authenticated user or a specific owner. */
export function searchGithubRepos(query: string, owner?: string): Promise<GitHubRepo[]> {
  return invokeCommand('search_github_repos', { query, owner: owner ?? null });
}

/** Check if a repository is likely a monorepo by counting modules in MODULES.yaml.
 *  Returns the module count (0 if file doesn't exist). */
export function checkMonorepoModules(githubRepo: string): Promise<number> {
  return invokeCommand('check_monorepo_modules', { githubRepo });
}

/** Validate that a subpath exists as a directory in a GitHub repository. */
export function validateSubpath(githubRepo: string, subpath: string): Promise<void> {
  return invokeCommand('validate_subpath', { githubRepo, subpath });
}

/** List directories at a given path in a GitHub repository. */
export function listRepoDirectories(githubRepo: string, path: string): Promise<string[]> {
  return invokeCommand('list_repo_directories', { githubRepo, path });
}

// =============================================================================
// Project notes & sessions
// =============================================================================

export function listProjectNotes(projectId: string): Promise<import('./types').ProjectNote[]> {
  return invokeCommand('list_project_notes', { projectId });
}

export function createProjectNote(
  projectId: string,
  title: string,
  content: string
): Promise<import('./types').ProjectNote> {
  return invokeCommand('create_project_note', { projectId, title, content });
}

/**
 * Fetch a single project note by id (no project filter). The
 * `#project-note:<id>` counterpart of {@link getNote}: used to open a reference
 * that points outside the current project's note list.
 */
export function getProjectNote(
  projectNoteId: string
): Promise<import('./types').ProjectNote | null> {
  return invokeCommand('get_project_note', { projectNoteId });
}

export function getProjectNoteBySession(
  sessionId: string
): Promise<import('./types').ProjectNote | null> {
  return invokeCommand('get_project_note_by_session', { sessionId });
}

export function deleteProjectNote(noteId: string): Promise<void> {
  return invokeCommand('delete_project_note', { noteId });
}

/**
 * Fetch a single note by id (no branch filter). Used to open a `#note:<id>`
 * hashtag reference from a rendered note body, including child notes that live
 * on other repo branches.
 */
export function getNote(noteId: string): Promise<import('./types').NoteTimelineItem | null> {
  return invokeCommand('get_note', { noteId });
}

/**
 * List the child notes aggregated under a given project note. Children are
 * hidden from branch timelines, so this is the dedicated path the project-note
 * view uses to resolve `#note:<id>` references to their titles.
 */
export function listChildNotes(
  parentProjectNoteId: string
): Promise<import('./types').NoteTimelineItem[]> {
  return invokeCommand('list_child_notes', { parentProjectNoteId });
}

export function startProjectSession(
  projectId: string,
  prompt: string,
  provider?: string,
  imageIds?: string[],
  acpConfigSelection?: AcpConfigSelection
): Promise<import('./types').ProjectSessionResponse> {
  return invokeCommand('start_project_session', {
    projectId,
    prompt,
    provider: provider ?? null,
    imageIds: imageIds?.length ? imageIds : null,
    acpConfigSelection: acpConfigSelection ?? null,
  });
}

// =============================================================================
// Branches
// =============================================================================

export function listBranchesForProject(
  projectId: string,
  options: ForceRefreshOptions = {}
): Promise<SwrResult<Branch[]>> {
  return cachedCommand(
    'list_branches_for_project',
    { projectId },
    {
      ttl: 2 * 60_000,
      bypassRead: options.force,
    }
  );
}

/** Get a single branch by ID. */
export function getBranch(branchId: string): Promise<Branch | null> {
  return invokeCommand('get_branch', { branchId });
}

/** Create a local branch record (DB only — no git worktree yet).
 *  Returns immediately with worktreePath = null.
 *  Call `setupWorktree` separately to create the git worktree. */
export async function createBranch(
  projectId: string,
  branchName: string,
  baseBranch?: string,
  projectRepoId?: string
): Promise<Branch> {
  const branch = await invokeCommand<Branch>('create_branch', {
    projectId,
    branchName,
    baseBranch,
    projectRepoId,
  });
  await invalidateCacheByCommand('list_branches_for_project');
  return branch;
}

/** Create the git worktree for a local branch and record its workdir.
 *  Returns the updated branch with worktreePath populated. */
export function setupWorktree(branchId: string): Promise<Branch> {
  return invokeCommand('setup_worktree', { branchId });
}

/** Like setupWorktree, but also runs prerun actions after the worktree is ready.
 *  Used by the retry path so prerun actions aren't skipped on recovery. */
export function setupWorktreeAndRunPrerun(branchId: string, provider?: string): Promise<Branch> {
  return invokeCommand('setup_worktree_and_run_prerun', { branchId, provider });
}

/** Import a GitHub PR: fetch its head ref, create a local branch + worktree,
 *  and record everything in the DB. Returns the branch with worktreePath
 *  already populated (no separate setupWorktree call needed). */
export function setupWorktreeFromPr(
  projectId: string,
  prNumber: number,
  headRef: string,
  baseRef: string,
  projectRepoId?: string
): Promise<Branch> {
  return invokeCommand('setup_worktree_from_pr', {
    projectId,
    prNumber,
    headRef,
    baseRef,
    projectRepoId,
  });
}

/** Create a remote branch record (does not start the workspace). */
export function createRemoteBranch(
  projectId: string,
  branchName: string,
  workspaceName: string,
  baseBranch?: string,
  projectRepoId?: string
): Promise<Branch> {
  return invokeCommand('create_remote_branch', {
    projectId,
    branchName,
    baseBranch,
    workspaceName,
    projectRepoId,
  });
}

/** Start the Blox workspace for a remote branch. */
export function startWorkspace(branchId: string): Promise<void> {
  return invokeCommand('start_workspace', { branchId });
}

/** Resume a suspended Blox workspace. Returns IDs of all affected branches. */
export function resumeWorkspace(workspaceName: string): Promise<string[]> {
  return invokeCommand('resume_workspace', { workspaceName });
}

export async function deleteBranch(branchId: string): Promise<void> {
  await invokeCommand('delete_branch', { branchId });
  await Promise.all([
    invalidateCacheByCommand('list_branches_for_project'),
    invalidateCache('get_branch_timeline', { branchId }),
  ]);
}

export function renameBranch(branchId: string, branchName: string): Promise<Branch> {
  return invokeCommand('rename_branch', { branchId, branchName });
}

/**
 * Move a branch into another project, taking its notes, commits, reviews,
 * sessions and images with it. Both projects must be local.
 *
 * A move reshapes both ends — each project's branch list, its repos, and the
 * `github_repo` each one denormalizes from its primary — so all three caches go
 * stale, the way `deleteProject` treats them.
 */
export async function moveBranch(branchId: string, targetProjectId: string): Promise<Branch> {
  const branch = await invokeCommand<Branch>('move_branch', { branchId, targetProjectId });
  await Promise.all([
    invalidateCacheByCommand('list_projects'),
    invalidateCacheByCommand('list_branches_for_project'),
    invalidateCacheByCommand('list_project_repos'),
  ]);
  return branch;
}

/** Return the BLOX_ENV environment variable value, or null if unset. */
export function getBloxEnv(): Promise<string | null> {
  return invokeCommand('get_blox_env');
}

/** Get info about a remote branch's Blox workspace. */
export function getWorkspaceInfo(branchId: string): Promise<WorkspaceInfo> {
  return invokeCommand('get_workspace_info', { branchId });
}

/** Poll a remote branch's workspace status and return updated status + workstation ID. */
export function pollWorkspaceStatus(branchId: string): Promise<PollWorkspaceResult> {
  return invokeCommand('poll_workspace_status', { branchId });
}

/** Poll workspace statuses for multiple branches in a single `sq blox ws list` call. */
export function pollAllWorkspaceStatuses(
  branchIds: string[]
): Promise<Record<string, PollWorkspaceResult>> {
  return invokeCommand('poll_all_workspace_statuses', { branchIds });
}

// =============================================================================
// Timeline
// =============================================================================

const TIMELINE_FRESH_MS = 10_000;
const TIMELINE_CACHE_TTL = 30_000;
const timelineCache = new Map<string, { timeline: BranchTimeline; fetchedAt: number }>();
const inFlightTimelines = new Map<string, Promise<BranchTimeline>>();

/** Cap snapshot size so it stays comfortably under the localStorage quota. */
const MAX_SNAPSHOT_TIMELINES = 40;

type TimelineCacheEntry = { timeline: BranchTimeline; fetchedAt: number };

/**
 * Seed the in-memory timeline cache synchronously from the previous session's
 * localStorage snapshot. This runs at module init (before any BranchCard mounts)
 * so cached timelines can paint on the first frame of a cold iOS reload, instead
 * of each card awaiting its own asynchronous IndexedDB read. IndexedDB remains
 * the source of truth; the snapshot is only a paint-on-first-frame accelerator.
 * The seeded entries carry their original `fetchedAt`, so they read as stale and
 * `getBranchTimelineWithRevalidation` still kicks off a fresh fetch.
 */
function seedTimelineCacheFromSnapshot(): void {
  const snapshot = readSnapshot<Record<string, TimelineCacheEntry>>(SNAPSHOT_KEYS.timelines);
  if (!snapshot) return;
  for (const [branchId, entry] of Object.entries(snapshot)) {
    if (entry?.timeline && !timelineCache.has(branchId)) {
      timelineCache.set(branchId, entry);
    }
  }
}
seedTimelineCacheFromSnapshot();

/**
 * Persist a compact snapshot of the in-memory timeline cache to localStorage so
 * the next cold boot can re-seed it synchronously. Called from the page
 * lifecycle listener right before the tab is hidden/torn down (the last moment
 * we can capture state synchronously on iOS). Keeps only the most recently
 * fetched entries to bound the serialized size.
 */
export function persistTimelineSnapshot(): void {
  if (isTauri || timelineCache.size === 0) return;
  const snapshot: Record<string, TimelineCacheEntry> = Object.fromEntries(
    [...timelineCache.entries()]
      .sort((a, b) => b[1].fetchedAt - a[1].fetchedAt)
      .slice(0, MAX_SNAPSHOT_TIMELINES)
  );
  writeSnapshot(SNAPSHOT_KEYS.timelines, snapshot);
}

/**
 * Eagerly warm the in-memory timeline cache for every branch of a project, in
 * parallel. Called during boot for the restored project so cards find data
 * ready instead of each kicking off its own serialized IndexedDB read after
 * `ProjectHome` mounts. Requests dedupe against `inFlightTimelines`, so this is
 * safe to run alongside the per-card reads.
 */
export async function warmProjectTimelines(projectId: string): Promise<void> {
  if (isTauri) return;
  try {
    const { data: branches } = await listBranchesForProject(projectId);
    await Promise.allSettled(branches.map((b) => getBranchTimeline(b.id)));
  } catch {
    // Best-effort warm — cards fall back to their own lazy reads.
  }
}

/**
 * Drop a branch's timeline from every cache layer and tell open views to
 * refetch.
 *
 * Don't call this after a store-backed mutation — those publish
 * branch-changed / notes-changed / review-changed, and
 * cacheInvalidationListener already invalidates centrally in every window.
 * The remaining call sites cover what the change feed deliberately doesn't:
 * git-only worktree operations (reset, discard) and session-family writes
 * (queueing or cancelling sessions), which publish nothing.
 */
export function invalidateBranchTimeline(branchId: string): void {
  timelineCache.delete(branchId);
  inFlightTimelines.delete(branchId);
  invalidateCache('get_branch_timeline', { branchId });
  window.dispatchEvent(
    new CustomEvent('timeline-invalidated', { detail: { branchIds: [branchId] } })
  );
}

/**
 * Drop *every* branch's timeline and tell all open views to refetch — the
 * blunt version of `invalidateBranchTimeline` for when we don't know which
 * branches changed. Reserved for the store change feed's lag recovery
 * (`branch-changed` with a null branchId); nothing else should need it.
 */
export function invalidateAllBranchTimelines(): void {
  timelineCache.clear();
  inFlightTimelines.clear();
  invalidateCacheByCommand('get_branch_timeline');
  // `branchIds: null` means "all" — every listening card reloads.
  window.dispatchEvent(new CustomEvent('timeline-invalidated', { detail: { branchIds: null } }));
}

interface GetBranchTimelineOptions {
  force?: boolean;
}

export function getBranchTimeline(
  branchId: string,
  { force = false }: GetBranchTimelineOptions = {}
): Promise<BranchTimeline> {
  if (!force) {
    const cached = timelineCache.get(branchId);
    if (cached) {
      return Promise.resolve(cached.timeline);
    }

    const existing = inFlightTimelines.get(branchId);
    if (existing) {
      return existing;
    }
  }

  // Use cachedInvoke so IndexedDB serves data on cold start while the network
  // fetch runs in parallel (SWR). The first yield may be cached; the last is
  // always the freshest available value.
  let request: Promise<BranchTimeline> | undefined;
  const timelineRequest = (async () => {
    let timeline: BranchTimeline | undefined;
    for await (const { data, fetchedAt } of cachedInvoke<BranchTimeline>(
      'get_branch_timeline',
      { branchId },
      { ttl: TIMELINE_CACHE_TTL, bypassRead: force }
    )) {
      timeline = data;
      if (request && inFlightTimelines.get(branchId) === request) {
        timelineCache.set(branchId, { timeline, fetchedAt });
      }
    }
    return timeline!;
  })();

  request = timelineRequest.finally(() => {
    if (request && inFlightTimelines.get(branchId) === request) {
      inFlightTimelines.delete(branchId);
    }
  });

  inFlightTimelines.set(branchId, request);
  return request;
}

export function getBranchTimelineWithRevalidation(branchId: string): {
  cached: BranchTimeline | null;
  fresh: Promise<BranchTimeline> | null;
} {
  const entry = timelineCache.get(branchId);
  if (!entry) {
    return { cached: null, fresh: getBranchTimeline(branchId) };
  }
  const isFresh = Date.now() - entry.fetchedAt < TIMELINE_FRESH_MS;
  return {
    cached: entry.timeline,
    fresh: isFresh ? null : getBranchTimeline(branchId, { force: true }),
  };
}

export function refreshBranchGitState(
  branchId: string,
  options: { force?: boolean } = {}
): Promise<void> {
  return invokeCommand('refresh_branch_git_state', {
    branchId,
    force: options.force ?? false,
  });
}

export interface ParentBranchCommit {
  sha: string;
  shortSha: string;
  subject: string;
  author: string;
  authorEmail: string;
  timestamp: number;
}

export function listParentBranchCommits(branchId: string): Promise<ParentBranchCommit[]> {
  return invokeCommand('list_parent_branch_commits', { branchId });
}

export function invalidateProjectBranchTimelines(branchIds: string[]): void {
  for (const id of branchIds) {
    timelineCache.delete(id);
    inFlightTimelines.delete(id);
    invalidateCache('get_branch_timeline', { branchId: id });
  }
  window.dispatchEvent(new CustomEvent('timeline-invalidated', { detail: { branchIds } }));
}

/** Fast-forward a branch to origin, or queue the pull behind in-flight branch
 *  sessions. The backend owns that decision: it resolves to the queued session's
 *  ID when the pull has to wait, and to `null` when the pull already ran. */
export function pullOrQueueBranch(branchId: string): Promise<string | null> {
  return invokeCommand('pull_or_queue_branch', { branchId });
}

export function resetBranchToRemote(branchId: string): Promise<void> {
  return invokeCommand('reset_branch_to_remote', { branchId });
}

// =============================================================================
// Actions
// =============================================================================

export interface ProjectAction {
  id: string;
  contextId: string;
  name: string;
  command: string;
  actionType: string;
  sortOrder: number;
  autoCommit: boolean;
  createdAt: number;
  updatedAt: number;
}

export function listProjectActions(
  projectId: string,
  projectRepoId?: string | null
): Promise<ProjectAction[]> {
  return invokeCommand('list_project_actions', { projectId, projectRepoId: projectRepoId ?? null });
}

export function updateProjectAction(
  actionId: string,
  name: string,
  command: string,
  actionType: string,
  sortOrder: number,
  autoCommit: boolean
): Promise<void> {
  return invokeCommand('update_project_action', {
    actionId,
    name,
    command,
    actionType,
    sortOrder,
    autoCommit,
  });
}

export function deleteProjectAction(actionId: string): Promise<void> {
  return invokeCommand('delete_project_action', { actionId });
}

export interface ActionContext {
  id: string;
  githubRepo: string;
  subpath: string | null;
  hasDetectedActions: boolean;
  detectingActions: boolean;
  createdAt: number;
  updatedAt: number;
}

export function listActionContexts(): Promise<ActionContext[]> {
  return invokeCommand('list_action_contexts');
}

export function listRepoActions(githubRepo: string, subpath?: string): Promise<ProjectAction[]> {
  return invokeCommand('list_repo_actions', { githubRepo, subpath: subpath ?? null });
}

/**
 * Build the synthetic scope id under which repo-scoped action executions are
 * routed: `repo:{githubRepo}` or `repo:{githubRepo}:{subpath}`. Running-action
 * queries and action events carry it where branch runs carry a branch id.
 * Mirrors `repo_action_scope_id` in src-tauri/src/actions/commands.rs.
 */
export function repoActionScopeId(githubRepo: string, subpath?: string): string {
  return subpath ? `repo:${githubRepo}:${subpath}` : `repo:${githubRepo}`;
}

/**
 * Run a repo-scoped action against the repo's local clone.
 * Requires the local clone to exist on disk, and always disables auto-commit
 * (the working dir is the user's default-branch checkout, not a worktree).
 * Returns an execution ID routed under `repoActionScopeId(githubRepo, subpath)`.
 */
export function runRepoAction(
  githubRepo: string,
  subpath: string | undefined,
  actionId: string,
  provider?: string
): Promise<string> {
  return invokeCommand('run_repo_action', {
    githubRepo,
    subpath: subpath ?? null,
    actionId,
    provider: provider ?? null,
  });
}

export function createRepoAction(
  githubRepo: string,
  subpath: string | undefined,
  name: string,
  command: string,
  actionType: string,
  sortOrder: number,
  autoCommit: boolean
): Promise<ProjectAction> {
  return invokeCommand('create_repo_action', {
    githubRepo,
    subpath: subpath ?? null,
    name,
    command,
    actionType,
    sortOrder,
    autoCommit,
  });
}

export function deleteAllRepoActions(contextId: string): Promise<void> {
  return invokeCommand('delete_all_repo_actions', { contextId });
}

export function deleteActionContext(contextId: string): Promise<void> {
  return invokeCommand('delete_action_context', { contextId });
}

// =============================================================================
// Utilities
// =============================================================================

/** Open a URL in the user's default browser. */
export function openUrl(url: string): Promise<void> {
  if (!isTauri) {
    const opened = window.open(url, '_blank');
    if (!opened) {
      window.location.assign(url);
    } else {
      opened.opener = null;
    }
    return Promise.resolve();
  }

  return invokeCommand('open_url', { url });
}

/** Read a text file from an absolute path (used for Tauri native drag-and-drop). */
export function readTextFile(filePath: string): Promise<string> {
  return invokeCommand('read_text_file', { filePath });
}

/** Resolve equivalent display roots for paths that may involve symlinks. */
export function resolvePathAliases(paths: string[]): Promise<string[]> {
  return invokeCommand('resolve_path_aliases', { paths });
}

/** Intercept link clicks so they open in the system browser, not the webview. */
export function handleExternalLinkClick(e: MouseEvent): void {
  const anchor = (e.target as HTMLElement).closest('a');
  if (!anchor) return;
  const href = anchor.getAttribute('href');
  if (href && (href.startsWith('http://') || href.startsWith('https://'))) {
    e.preventDefault();
    openUrl(href);
  }
}

// =============================================================================
// Agent discovery
// =============================================================================

export interface AcpProviderInfo {
  id: string;
  label: string;
}

export interface AcpConfigValueOption {
  valueId: string;
  label: string;
  groupLabel?: string | null;
}

export interface AcpConfigSelector {
  configId: string;
  label: string;
  currentValueId: string;
  options: AcpConfigValueOption[];
}

export interface AcpConfigDiscovery {
  providerId: string;
  model?: AcpConfigSelector | null;
  effort?: AcpConfigSelector | null;
}

export interface DiscoverAcpProvidersOptions {
  force?: boolean;
}

export interface DiscoverAcpConfigOptions {
  force?: boolean;
  selectedModelValue?: string | null;
}

const ACP_PROVIDER_CACHE_TTL = 30 * 60_000;

/** Scan the system for installed ACP-compatible agents. */
export function discoverAcpProviders(
  options: DiscoverAcpProvidersOptions = {}
): Promise<SwrResult<AcpProviderInfo[]>> {
  return cachedCommand('discover_acp_providers', undefined, {
    ttl: options.force ? 0 : ACP_PROVIDER_CACHE_TTL,
  });
}

/** Discover model/effort selectors exposed by a provider for a working directory. */
export async function discoverAcpConfig(
  providerId: string,
  workingDir?: string | null,
  options: DiscoverAcpConfigOptions = {}
): Promise<SwrResult<AcpConfigDiscovery>> {
  const data = await invokeCommand<AcpConfigDiscovery>('discover_acp_config', {
    providerId,
    workingDir: workingDir ?? null,
    force: options.force ?? false,
    selectedModelValue: options.selectedModelValue ?? null,
  });
  return { data, revalidating: null };
}

// =============================================================================
// Sessions
// =============================================================================

export function getSession(sessionId: string): Promise<Session | null> {
  return invokeCommand('get_session', { sessionId });
}

/**
 * Busy-state snapshot: all running and queued sessions projected to their
 * branch/project context. Clients hydrate from this on load or reconnect,
 * then apply `session-status-changed` deltas on top.
 */
export function getActiveSessions(): Promise<ActiveSessionInfo[]> {
  return invokeCommand('get_active_sessions');
}

export function getSessionMessages(sessionId: string): Promise<SwrResult<SessionMessage[]>> {
  return cachedCommand('get_session_messages', { sessionId }, { ttl: 5 * 60_000 });
}

/** Fetch session messages without SWR cache, for terminal status handlers. */
export function getFreshSessionMessages(sessionId: string): Promise<SessionMessage[]> {
  return invokeCommand('get_session_messages', { sessionId });
}

export function getSessionMessagesSince(
  sessionId: string,
  sinceId: number
): Promise<SessionMessage[]> {
  return invokeCommand('get_session_messages_since', { sessionId, sinceId });
}

export function getSessionAcpMetadataMessages(sessionId: string): Promise<SessionMessage[]> {
  return invokeCommand('get_session_acp_metadata_messages', { sessionId });
}

/**
 * Incremental variant of getSessionAcpMetadataMessages: returns metadata rows
 * with id > sinceId plus fresh copies of refetchIds — rows the caller knows
 * may still be mutated in place (tool calls without a terminal status).
 */
export function getSessionAcpMetadataMessagesSince(
  sessionId: string,
  sinceId: number,
  refetchIds: number[]
): Promise<SessionMessage[]> {
  return invokeCommand('get_session_acp_metadata_messages_since', {
    sessionId,
    sinceId,
    refetchIds,
  });
}

export function countAssistantMessagesAfter(
  sessionId: string,
  afterTimestamp: number
): Promise<number> {
  return invokeCommand('count_assistant_messages_after', { sessionId, afterTimestamp });
}

/** Create a session and immediately start the agent. */
export function startSession(
  prompt: string,
  workingDir: string,
  provider?: string,
  acpConfigSelection?: AcpConfigSelection
): Promise<Session> {
  return invokeCommand('start_session', {
    prompt,
    workingDir,
    provider: provider ?? null,
    acpConfigSelection: acpConfigSelection ?? null,
  });
}

/** Send a follow-up message to an existing session.
 *  The backend uses the provider that originally created the session. */
export function resumeSession(
  sessionId: string,
  prompt: string,
  imageIds?: string[],
  branchId?: string | null,
  acpConfigSelection?: AcpConfigSelection
): Promise<void> {
  return invokeCommand('resume_session', {
    sessionId,
    prompt,
    imageIds: imageIds ?? null,
    branchId: branchId ?? null,
    acpConfigSelection: acpConfigSelection ?? null,
  });
}

export function queueSessionMessage(
  sessionId: string,
  content: string,
  imageIds?: string[],
  branchId?: string | null
): Promise<QueuedSessionMessage> {
  return invokeCommand('queue_session_message', {
    sessionId,
    content,
    imageIds: imageIds ?? null,
    branchId: branchId ?? null,
  });
}

export function listQueuedSessionMessages(sessionId: string): Promise<QueuedSessionMessage[]> {
  return invokeCommand('list_queued_session_messages', { sessionId });
}

export function deleteQueuedSessionMessage(id: string): Promise<boolean> {
  return invokeCommand('delete_queued_session_message', { id });
}

export function sendQueuedSessionMessage(id: string): Promise<void> {
  return invokeCommand('send_queued_session_message', { id });
}

export function buildNoteFollowupMessage(
  sessionId: string,
  branchId: string | null | undefined,
  hasParsedNote: boolean
): Promise<string> {
  return invokeCommand('build_note_followup_message', {
    sessionId,
    branchId: branchId ?? null,
    hasParsedNote,
  });
}

export function cancelSession(sessionId: string): Promise<void> {
  return invokeCommand('cancel_session', { sessionId });
}

export function deleteSession(sessionId: string): Promise<void> {
  return invokeCommand('delete_session', { sessionId });
}

/** Start a branch-scoped session (note or commit). */
export function startBranchSession(
  branchId: string,
  prompt: string,
  sessionType: BranchSessionType,
  provider?: string,
  imageIds?: string[],
  launchContext?: BranchSessionLaunchContext,
  acpConfigSelection?: AcpConfigSelection
): Promise<BranchSessionResponse> {
  return invokeCommand('start_branch_session', {
    branchId,
    prompt,
    sessionType,
    provider: provider ?? null,
    imageIds: imageIds ?? null,
    launchContext: launchContext ?? null,
    acpConfigSelection: acpConfigSelection ?? null,
  });
}

/** Start a branch session immediately when compatible, otherwise queue it. */
export function startOrQueueBranchSession(
  branchId: string,
  prompt: string,
  sessionType: BranchSessionType,
  provider?: string,
  imageIds?: string[],
  launchContext?: BranchSessionLaunchContext,
  acpConfigSelection?: AcpConfigSelection
): Promise<BranchSessionResponse> {
  return invokeCommand('start_or_queue_branch_session', {
    branchId,
    prompt,
    sessionType,
    provider: provider ?? null,
    imageIds: imageIds ?? null,
    launchContext: launchContext ?? null,
    acpConfigSelection: acpConfigSelection ?? null,
  });
}

/** Queue a branch-scoped session to run after current work completes. */
export function queueBranchSession(
  branchId: string,
  prompt: string,
  sessionType: BranchSessionType,
  provider?: string,
  imageIds?: string[],
  launchContext?: BranchSessionLaunchContext,
  acpConfigSelection?: AcpConfigSelection
): Promise<BranchSessionResponse> {
  return invokeCommand('queue_branch_session', {
    branchId,
    prompt,
    sessionType,
    provider: provider ?? null,
    imageIds: imageIds ?? null,
    launchContext: launchContext ?? null,
    acpConfigSelection: acpConfigSelection ?? null,
  });
}

/** Drain queued sessions for a branch — starts the next queued session if any.
 *  Returns true if a session was started, false if the queue was empty. */
export function drainQueuedSessions(branchId: string): Promise<boolean> {
  return invokeCommand('drain_queued_sessions', {
    branchId,
    provider: null,
  });
}

// =============================================================================
// Timeline item deletion
// =============================================================================

/** Create a standalone note (no session) for a branch. */
export function createNote(
  branchId: string,
  title: string,
  content: string
): Promise<{ id: string; title: string; content: string; createdAt: number; updatedAt: number }> {
  return invokeCommand('create_note', { branchId, title, content });
}

/** Delete a note and optionally its linked session. */
export function deleteNote(noteId: string, deleteSession = true): Promise<void> {
  return invokeCommand('delete_note', { noteId, deleteSession });
}

/** Delete a commit (git reset --hard to parent) and optionally its session.
 *  Only works for the tip commit (HEAD) of the branch. */
export function deleteCommit(
  branchId: string,
  commitSha: string,
  deleteSession = true
): Promise<void> {
  return invokeCommand('delete_commit', { branchId, commitSha, deleteSession });
}

/** Delete a pending commit (no SHA) by its DB id, optionally its session. */
export function deletePendingCommit(commitId: string, deleteSession = true): Promise<void> {
  return invokeCommand('delete_pending_commit', { commitId, deleteSession });
}

/** Preview the exact worktree paths that would be reverted or removed. */
export function getWorktreeChangesPreview(branchId: string): Promise<WorktreeChangesPreview> {
  return invokeCommand('get_worktree_changes_preview', { branchId });
}

/** Discard all uncommitted worktree changes after backend safety checks. */
export function discardWorktreeChanges(
  branchId: string,
  expectedPreview?: WorktreeChangesPreview
): Promise<void> {
  return invokeCommand('discard_worktree_changes', { branchId, expectedPreview });
}

/** Delete a review and all its comments, optionally its linked session. */
export function deleteReview(reviewId: string, deleteSession = true): Promise<void> {
  return invokeCommand('delete_review', { reviewId, deleteSession });
}

// =============================================================================
// Diff
// =============================================================================

/**
 * List files changed in a branch or commit diff.
 *
 * For branch scope: merge-base(base, tip)..tip
 * For commit scope: parent..sha
 *
 * `commitSha` is optional for branch scope (resolves to current tip).
 * Returns the resolved SHA alongside the file list.
 */
export function getDiffFiles(
  branchId: string,
  commitSha?: string,
  scope: DiffScope = 'branch'
): Promise<SwrResult<DiffFilesResponse>> {
  return cachedCommand('get_diff_files', { branchId, commitSha, scope }, { ttl: 2 * 60_000 });
}

/** Get the full diff content for a single file. */
export function getFileDiff(
  branchId: string,
  commitSha: string,
  scope: DiffScope,
  path: string
): Promise<SwrResult<FileDiff>> {
  return cachedCommand('get_file_diff', { branchId, commitSha, scope, path }, { ttl: 2 * 60_000 });
}

/** Get file content at a specific ref (for reference files). */
export function getFileAtRef(branchId: string, refName: string, path: string): Promise<File> {
  return invokeCommand('get_file_at_ref', { branchId, refName, path });
}

// =============================================================================
// Review
// =============================================================================

/**
 * Get or create a review for a branch + commit + scope.
 * Lazy creation — only called on first persistent action.
 */
export function ensureReview(
  branchId: string,
  commitSha: string,
  scope: 'branch' | 'commit'
): Promise<Review> {
  return invokeCommand('ensure_review', { branchId, commitSha, scope });
}

/** Find an existing review by (branch, commit, scope) without creating one. */
export function findReview(
  branchId: string,
  commitSha: string,
  scope: 'branch' | 'commit'
): Promise<Review | null> {
  return invokeCommand('find_review', { branchId, commitSha, scope });
}

/** Get a review by ID with all child data. */
export function getReview(reviewId: string): Promise<Review | null> {
  return invokeCommand('get_review', { reviewId });
}

/** Mark a file as reviewed. */
export function markReviewed(reviewId: string, path: string): Promise<void> {
  return invokeCommand('mark_reviewed', { reviewId, path });
}

/** Unmark a file as reviewed. */
export function unmarkReviewed(reviewId: string, path: string): Promise<void> {
  return invokeCommand('unmark_reviewed', { reviewId, path });
}

/** Add a comment to a review. */
export function addComment(
  reviewId: string,
  path: string,
  spanStart: number,
  spanEnd: number,
  content: string
): Promise<Comment> {
  return invokeCommand('add_comment', { reviewId, path, spanStart, spanEnd, content });
}

/** Update a comment's content. */
export function updateComment(commentId: string, content: string): Promise<void> {
  return invokeCommand('update_comment', { commentId, content });
}

/** Link a review comment to the note/commit session started from its button.
 *  The two links are independent, so a single comment can have both. */
export function linkCommentSession(
  commentId: string,
  sessionId: string,
  sessionType: 'note' | 'commit'
): Promise<void> {
  return invokeCommand('link_comment_session', { commentId, sessionId, sessionType });
}

/** Resolve the branch note produced by a session (for NoteModal).
 *  Returns null if no note is linked to the session. */
export function getBranchNoteBySession(sessionId: string): Promise<BranchNote | null> {
  return invokeCommand('get_branch_note_by_session', { sessionId });
}

/** Resolve the commit SHA produced by a session.
 *  Returns null if no commit is linked, or its SHA is still pending. */
export function getBranchCommitBySession(sessionId: string): Promise<string | null> {
  return invokeCommand('get_branch_commit_by_session', { sessionId });
}

/** Delete a comment (soft delete). */
export function deleteComment(commentId: string): Promise<void> {
  return invokeCommand('delete_comment', { commentId });
}

/** Soft-delete all active comments for a review in one atomic operation. */
export function deleteAllComments(reviewId: string): Promise<void> {
  return invokeCommand('delete_all_comments', { reviewId });
}

/** Restore a soft-deleted comment. */
export function restoreComment(commentId: string): Promise<void> {
  return invokeCommand('restore_comment', { commentId });
}

/** Get soft-deleted comments for a review. */
export function getDeletedComments(reviewId: string): Promise<Comment[]> {
  return invokeCommand('get_deleted_comments', { reviewId });
}

/** Add a reference file to a review. */
export function addReferenceFile(reviewId: string, path: string): Promise<void> {
  return invokeCommand('add_reference_file', { reviewId, path });
}

/** Remove a reference file from a review. */
export function removeReferenceFile(reviewId: string, path: string): Promise<void> {
  return invokeCommand('remove_reference_file', { reviewId, path });
}

// =============================================================================
// Git helpers
// =============================================================================

/** Check whether the `sq` CLI is available on this system. */
export function isSqAvailable(): Promise<boolean> {
  return invokeCommand('is_sq_available');
}

/** Check whether the user is authenticated with Blox.
 *  Resolves if authenticated, rejects with an error message if not. */
export function checkBloxAuth(): Promise<void> {
  return invokeCommand('check_blox_auth');
}

export function listGitBranches(githubRepo: string): Promise<BranchRef[]> {
  return invokeCommand('list_git_branches', { githubRepo });
}

export function detectDefaultBranch(githubRepo: string): Promise<string> {
  return invokeCommand('detect_default_branch_cmd', { githubRepo });
}

/** Prune stale remote-tracking refs in the background.
 *  With GitHub-repo-based projects, this is a no-op. */
export function pruneRemoteRefs(githubRepo: string): Promise<void> {
  return invokeCommand('prune_remote_refs', { githubRepo });
}

/** Check whether a branch already exists locally for this project. */
export function checkExistingLocalBranch(projectId: string, branchName: string): Promise<boolean> {
  return invokeCommand('check_existing_local_branch', { projectId, branchName });
}

/** Fetch a single PR by number. Throws if not found. */
export function getPrForRepo(githubRepo: string, prNumber: number): Promise<PullRequest> {
  return invokeCommand('get_pr_for_repo', { githubRepo, prNumber });
}

/** Find the open PR (if any) whose head branch matches `branchName`. */
export function getPrForBranch(
  githubRepo: string,
  branchName: string
): Promise<PullRequest | null> {
  return invokeCommand('get_pr_for_branch', { githubRepo, branchName });
}

export function listPullRequests(githubRepo: string): Promise<PullRequest[]> {
  return invokeCommand('list_pull_requests', { githubRepo });
}

/** If the repo is a fork, return the parent repo slug (e.g. `"base-owner/repo"`). */
export function getParentRepo(githubRepo: string): Promise<string | null> {
  return invokeCommand('get_parent_repo', { githubRepo });
}

export function listIssues(githubRepo: string): Promise<Issue[]> {
  return invokeCommand('list_issues', { githubRepo });
}

// =============================================================================
// PR creation
// =============================================================================

/** Kick off an agent session to push the branch and create a PR via `gh`.
 *  Returns the session ID so the frontend can track progress.
 *  When `draft` is true the PR is created with `--draft`. */
export function createPr(branchId: string, provider?: string, draft?: boolean): Promise<string> {
  return invokeCommand('create_pr', { branchId, provider: provider ?? null, draft: draft ?? null });
}

/** Build the GitHub PR URL from the repo's origin remote and a PR number. */
export function getPrUrl(branchId: string, prNumber: number): Promise<string> {
  return invokeCommand('get_pr_url', { branchId, prNumber });
}

/** Update the PR number stored for a branch. */
export function updateBranchPr(branchId: string, prNumber: number | null): Promise<void> {
  return invokeCommand('update_branch_pr', { branchId, prNumber });
}

/** Look up an existing open PR for a branch on GitHub and persist it.
 *  Returns the recovered PR number, or null if no PR exists. */
export function recoverBranchPr(branchId: string): Promise<number | null> {
  return invokeCommand('recover_branch_pr', { branchId });
}

/** Check whether a branch has local commits not yet pushed to the remote. */
export function hasUnpushedCommits(branchId: string): Promise<boolean> {
  return invokeCommand('has_unpushed_commits', { branchId });
}

/** Push a branch to its remote via an agent session.
 *  The agent runs git push and can fix pre-push hook failures.
 *  Queues behind in-flight branch sessions, so the response reports whether the
 *  returned session is pushing now or waiting on the branch queue. */
export function pushBranch(
  branchId: string,
  provider?: string,
  force?: boolean
): Promise<BranchPipelineResponse> {
  return invokeCommand('push_branch', {
    branchId,
    provider: provider ?? null,
    force: force ?? null,
  });
}

/** Post a single review comment to a GitHub PR. */
export function postCommentToGithub(
  branchId: string,
  prNumber: number,
  comment: Comment
): Promise<GitHubCommentResult> {
  return invokeCommand('post_comment_to_github', { branchId, prNumber, comment });
}

export interface GitHubCommentResult {
  commentUrl: string;
  commentId: number;
  commentType: string;
  /** The comment joined an existing pending review, so it isn't published yet. */
  pending: boolean;
}

/** Rebase a branch via a pipeline.
 *  When target is 'base' (default), rebases onto origin/{base_branch}.
 *  When target is 'origin', rebases onto origin/{branch_name}.
 *  Queues behind in-flight branch sessions, so the response reports whether the
 *  returned session is running or waiting on the branch queue. */
export function rebaseBranch(
  branchId: string,
  provider?: string,
  target?: 'base' | 'origin'
): Promise<BranchPipelineResponse> {
  return invokeCommand('rebase_branch', {
    branchId,
    provider: provider ?? null,
    target: target ?? null,
  });
}

/** Squash all commits on a branch into a single commit via a pipeline.
 *  Uses git reset --soft then hands off to AI to write the commit message.
 *  Queues behind in-flight branch sessions, so the response reports whether the
 *  returned session is running or waiting on the branch queue. */
export function squashCommits(
  branchId: string,
  provider?: string
): Promise<BranchPipelineResponse> {
  return invokeCommand('squash_commits', {
    branchId,
    provider: provider ?? null,
  });
}

// =============================================================================
// Doctor (Health Check)
// =============================================================================

export type DoctorFixType = 'command' | 'bridge' | 'auth' | 'updateMain' | 'updateBridge';

export type DoctorInstallSource =
  'brew' | 'npm' | 'cargo' | 'mise' | 'asdf' | 'curlPipe' | 'system' | 'bundled' | 'unknown';

/**
 * Version + install-source readout for one binary behind an agent check.
 *
 * An AI-agent check may front two distinct binaries — the agent's own CLI
 * (`main`) and its ACP bridge (`bridge`) — each versioned independently. Only
 * `installSource` is populated on the cheap path; the freshness pass fills in
 * the rest. Note `updateAvailable` is suppressed (null) for self-updating
 * tools, so treat null as "no actionable update", never as "up to date".
 */
export interface AgentVersionInfo {
  installSource: DoctorInstallSource | null;
  installedVersion: string | null;
  latestVersion: string | null;
  updateAvailable: boolean | null;
  selfUpdating: boolean | null;
  /** Source-aware update command. Non-null only when an update is actionable. */
  updateCommand: string | null;
  /** 'updateMain' or 'updateBridge', matching this readout's slot. */
  updateFixType: 'updateMain' | 'updateBridge' | null;
  /** True when this binary is managed by Staged (resolved from the managed
   *  bridge shim dir rather than a user install). Stamped by the doctor
   *  crate alongside installSource === 'bundled'. */
  bundled: boolean | null;
}

export interface DoctorCheck {
  id: string;
  label: string;
  status: 'pass' | 'warn' | 'fail';
  message: string;
  fixUrl: string | null;
  fixCommand: string | null;
  fixType: DoctorFixType | null;
  path: string | null;
  bridgePath: string | null;
  rawOutput: string | null;
  authStatus: 'authenticated' | 'notAuthenticated' | 'notApplicable' | 'unknown' | null;
  /** Flat version fields mirror the bridge readout (else main) for compat. */
  installedVersion: string | null;
  latestVersion: string | null;
  updateAvailable: boolean | null;
  installSource: DoctorInstallSource | null;
  selfUpdating: boolean | null;
  /** Independent readout for the agent's own CLI (e.g. `claude`, `codex`). */
  main: AgentVersionInfo | null;
  /** Independent readout for the agent's ACP bridge (e.g. `claude-agent-acp`). */
  bridge: AgentVersionInfo | null;
}

export interface DoctorReport {
  checks: DoctorCheck[];
}

/** Run all system health checks (cheap path — no version freshness). */
export function runDoctor(): Promise<DoctorReport> {
  return invokeCommand('run_doctor');
}

/**
 * Re-run all checks with version freshness enabled. Slower (hits npm/brew/
 * crates.io/GitHub) — call this as a second pass after `runDoctor` so the base
 * report paints instantly while version/update info fills in.
 */
export function runDoctorFreshness(): Promise<DoctorReport> {
  return invokeCommand('run_doctor_freshness');
}

/** Run a fix for a doctor check, identified by check ID and fix type. */
export function runDoctorFix(
  checkId: string,
  fixType: 'command' | 'bridge' | 'auth'
): Promise<void> {
  return invokeCommand('run_doctor_fix', { checkId, fixType });
}

/**
 * Run a source-aware update for a single readout (main CLI or ACP bridge).
 *
 * The backend re-derives the expected command for `(checkId, fixType)` and only
 * runs it if `command` matches, so this is not a raw-shell-exec path.
 */
export function runDoctorUpdate(
  checkId: string,
  fixType: 'updateMain' | 'updateBridge',
  command: string
): Promise<void> {
  return invokeCommand('run_doctor_update', { checkId, fixType, command });
}

// =============================================================================
// PR Status
// =============================================================================

/** Clear stale PR status fields for a branch (e.g. after a push invalidates them).
 *  Emits 'pr-status-cleared' event so the UI drops stale indicators immediately. */
export function clearBranchPrStatus(branchId: string): Promise<void> {
  return invokeCommand('clear_branch_pr_status', { branchId });
}

/** Refresh PR status for a specific branch. Emits 'pr-status-changed' event. */
export function refreshPrStatus(branchId: string): Promise<void> {
  return invokeCommand('refresh_pr_status', { branchId });
}

/** Refresh PR status for all branches with PRs. */
export function refreshAllPrStatuses(projectId: string): Promise<void> {
  return invokeCommand('refresh_all_pr_statuses', { projectId });
}

// ---------------------------------------------------------------------------
// PR poll scheduler — interest/hint commands
//
// The backend owns PR-polling cadence and concurrency. These commands feed it
// interest hints; the cadence, dedup, and failure backoff all live in the
// backend scheduler (see src-tauri/src/pr_poll_scheduler.rs).
//
// Interest is tracked per connected client, so every hint carries a `clientId`
// (see prPollingService.ts). The same id must also be sent on the WS connect
// query so the backend can correlate interest with disconnect.
// ---------------------------------------------------------------------------

/** Tell the backend which project this client has foregrounded/selected (→ selected tier). */
export function setForegroundProject(clientId: string, projectId: string | null): Promise<void> {
  return invokeCommand('set_foreground_project', { clientId, projectId });
}

/** Report this client's window focus to the backend. No focused client ⇒ polling pauses. */
export function setPrPollFocus(clientId: string, focused: boolean): Promise<void> {
  return invokeCommand('set_focus', { clientId, focused });
}

/** Mark whether a branch has pending CI checks for this client (→ pending tier for its project). */
export function setBranchPending(
  clientId: string,
  branchId: string,
  projectId: string,
  pending: boolean
): Promise<void> {
  return invokeCommand('set_branch_pending', { clientId, branchId, projectId, pending });
}

/** Nudge the backend to refresh a project's PR statuses now (folded into dedup). */
export function refreshPrStatusesNow(clientId: string, projectId: string): Promise<void> {
  return invokeCommand('refresh_now', { clientId, projectId });
}

/** Tell the backend this client has disconnected so its interest is dropped. */
export function disconnectPrPollClient(clientId: string): Promise<void> {
  return invokeCommand('disconnect_client', { clientId });
}

// =============================================================================
// Windows
// =============================================================================

/**
 * Open a new full-peer app window, optionally seeded with the opener's
 * selected project. Returns the new window's label.
 */
export function newWindow(seedProjectId: string | null): Promise<string> {
  return invokeCommand('new_window', { seedProjectId });
}

/**
 * One-shot read of the project this window was seeded with by its opener, if
 * any. Consumes the seed; only secondary (`win-*`) windows ever have one.
 */
export function takeWindowSeed(): Promise<string | null> {
  return invokeCommand('take_window_seed');
}

// =============================================================================
// Images
// =============================================================================

/** Upload a local image file and create a DB record for it. */
export function createImage(
  branchId: string | null,
  projectId: string,
  filePath: string,
  pending?: boolean
): Promise<Image> {
  if (!isTauri) {
    return Promise.reject(
      new Error('create_image is only available for desktop file paths; use createImageFromData')
    );
  }

  return invokeCommand('create_image', { branchId, projectId, filePath, pending });
}

/** Get the filesystem path for a stored image. */
export function getImagePath(imageId: string): Promise<string> {
  return invokeCommand('get_image_path', { imageId });
}

/** Delete an image record and its stored file. */
export function deleteImage(imageId: string): Promise<void> {
  return invokeCommand('delete_image', { imageId });
}

/** List all images for a branch. */
export function listBranchImages(branchId: string): Promise<Image[]> {
  return invokeCommand('list_branch_images', { branchId });
}

/** Get the base64-encoded data URL for an image. */
export function getImageData(imageId: string): Promise<string> {
  return invokeCommand('get_image_data', { imageId });
}

/** Create an image from base64-encoded data (browser file input / clipboard paste). */
export function createImageFromData(
  branchId: string | null,
  projectId: string,
  filename: string,
  mimeType: string,
  data: string,
  pending?: boolean
): Promise<Image> {
  return invokeCommand('create_image_from_data', {
    branchId,
    projectId,
    filename,
    mimeType,
    data,
    pending,
  });
}

// =============================================================================
// Repo Badges
// =============================================================================

/** Fetch all repo badges from the store. */
export function getAllRepoBadges(): Promise<import('./types').RepoBadge[]> {
  return invokeCommand('get_all_repo_badges');
}

/** Ensure badges exist for the given (githubRepo, subpath) pairs.
 *  Generates missing badges (fallback names + hues) and returns all requested. */
export function ensureRepoBadges(
  repos: [string, string][],
  provider?: string
): Promise<import('./types').RepoBadge[]> {
  return invokeCommand('ensure_repo_badges', { repos, provider: provider ?? null });
}

/** Update the short name and hue of an existing repo badge. */
export function updateRepoBadge(
  githubRepo: string,
  subpath: string,
  shortName: string,
  hue: number
): Promise<import('./types').RepoBadge> {
  return invokeCommand('update_repo_badge', { githubRepo, subpath, shortName, hue });
}

export function deleteRepoBadge(githubRepo: string, subpath: string): Promise<void> {
  return invokeCommand('delete_repo_badge', { githubRepo, subpath });
}

// =============================================================================
// Pinned Repos
// =============================================================================

/** Pin a repo badge so it appears prominently in the home screen and sidebar. */
export function pinRepo(githubRepo: string, subpath: string): Promise<void> {
  return invokeCommand('pin_repo', { githubRepo, subpath });
}

/** Unpin a repo badge. */
export function unpinRepo(githubRepo: string, subpath: string): Promise<void> {
  return invokeCommand('unpin_repo', { githubRepo, subpath });
}

/** Bulk-update pin sort order from an ordered list of [githubRepo, subpath] keys. */
export function reorderPinnedRepos(orderedKeys: [string, string][]): Promise<void> {
  return invokeCommand('reorder_pinned_repos', { orderedKeys });
}

/** List all repo badges ordered: pinned first by sort order, then unpinned by project count. */
export function listReposForHome(): Promise<import('./types').RepoHomeItem[]> {
  return invokeCommand('list_repos_for_home');
}

/** Store the detected default branch for a repo badge. */
export function setRepoDefaultBranch(
  githubRepo: string,
  subpath: string,
  defaultBranch: string
): Promise<void> {
  return invokeCommand('set_repo_default_branch', { githubRepo, subpath, defaultBranch });
}

/** Get the commit timeline for a repo's default branch from its local clone. */
export function getRepoDefaultBranchTimeline(
  githubRepo: string,
  subpath: string,
  limit?: number
): Promise<import('./types').RepoDefaultBranchTimeline> {
  return invokeCommand('get_repo_default_branch_timeline', { githubRepo, subpath, limit });
}

/** Clone a repo locally that has only been used remotely, then detect its default branch. */
export function cloneRepoLocally(githubRepo: string): Promise<string> {
  return invokeCommand('clone_repo_locally', { githubRepo });
}

/** Resolve the absolute filesystem path where a repo's local clone lives. */
export function getRepoClonePath(githubRepo: string): Promise<string> {
  return invokeCommand('get_repo_clone_path', { githubRepo });
}
