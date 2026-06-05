/**
 * Typed invoke wrappers for Tauri commands.
 *
 * One function per command. Each returns a typed promise.
 */

import { invokeCommand, isTauri } from './transport';
import type {
  Project,
  ProjectRepo,
  RecentRepo,
  GitHubRepo,
  Branch,
  BranchTimeline,
  BranchRef,
  BranchSessionLaunchContext,
  BranchSessionType,
  BranchSessionResponse,
  StoreIncompatibility,
  Session,
  SessionMessage,
  DiffFilesResponse,
  FileDiff,
  File,
  Review,
  Comment,
  WorkspaceInfo,
  PullRequest,
  Issue,
  PrStatus,
  PollWorkspaceResult,
  Image,
  SuggestedRepo,
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

export function listProjects(): Promise<Project[]> {
  return invokeCommand('list_projects');
}

export function createProject(
  name: string,
  location: 'local' | 'remote',
  githubRepo?: string,
  subpath?: string,
  branchName?: string,
  prNumber?: number,
  defaultBranch?: string,
  headRepo?: string
): Promise<Project> {
  return invokeCommand('create_project', {
    name,
    location,
    githubRepo: githubRepo ?? null,
    subpath,
    branchName: branchName ?? null,
    prNumber: prNumber ?? null,
    defaultBranch: defaultBranch ?? null,
    headRepo: headRepo ?? null,
  });
}

export function deleteProject(id: string): Promise<void> {
  return invokeCommand('delete_project', { id });
}

export function listProjectRepos(projectId: string): Promise<ProjectRepo[]> {
  return invokeCommand('list_project_repos', { projectId });
}

export function listRecentRepos(limit?: number): Promise<RecentRepo[]> {
  return invokeCommand('list_recent_repos', { limit: limit ?? 10 });
}

export function addProjectRepo(
  projectId: string,
  githubRepo: string,
  branchName?: string,
  subpath?: string,
  setAsPrimary?: boolean,
  prNumber?: number,
  defaultBranch?: string,
  headRepo?: string
): Promise<ProjectRepo> {
  return invokeCommand('add_project_repo', {
    projectId,
    githubRepo,
    branchName: branchName ?? null,
    subpath: subpath ?? null,
    setAsPrimary: setAsPrimary ?? null,
    prNumber: prNumber ?? null,
    defaultBranch: defaultBranch ?? null,
    headRepo: headRepo ?? null,
  });
}

export function updateProjectRepoBranchName(
  projectId: string,
  projectRepoId: string,
  branchName: string
): Promise<void> {
  return invokeCommand('update_project_repo_branch_name', { projectId, projectRepoId, branchName });
}

export function removeProjectRepo(projectId: string, projectRepoId: string): Promise<void> {
  return invokeCommand('remove_project_repo', { projectId, projectRepoId });
}

export function setPrimaryProjectRepo(projectId: string, projectRepoId: string): Promise<void> {
  return invokeCommand('set_primary_project_repo', { projectId, projectRepoId });
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

export function getProjectNoteBySession(
  sessionId: string
): Promise<import('./types').ProjectNote | null> {
  return invokeCommand('get_project_note_by_session', { sessionId });
}

export function deleteProjectNote(noteId: string): Promise<void> {
  return invokeCommand('delete_project_note', { noteId });
}

export function startProjectSession(
  projectId: string,
  prompt: string,
  provider?: string,
  imageIds?: string[]
): Promise<import('./types').ProjectSessionResponse> {
  return invokeCommand('start_project_session', {
    projectId,
    prompt,
    provider: provider ?? null,
    imageIds: imageIds?.length ? imageIds : null,
  });
}

// =============================================================================
// Branches
// =============================================================================

export function listBranchesForProject(projectId: string): Promise<Branch[]> {
  return invokeCommand('list_branches_for_project', { projectId });
}

/** Get a single branch by ID. */
export function getBranch(branchId: string): Promise<Branch | null> {
  return invokeCommand('get_branch', { branchId });
}

/** Create a local branch record (DB only — no git worktree yet).
 *  Returns immediately with worktreePath = null.
 *  Call `setupWorktree` separately to create the git worktree. */
export function createBranch(
  projectId: string,
  branchName: string,
  baseBranch?: string,
  projectRepoId?: string
): Promise<Branch> {
  return invokeCommand('create_branch', { projectId, branchName, baseBranch, projectRepoId });
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

export function deleteBranch(branchId: string): Promise<void> {
  return invokeCommand('delete_branch', { branchId });
}

export function renameBranch(branchId: string, branchName: string): Promise<Branch> {
  return invokeCommand('rename_branch', { branchId, branchName });
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
const timelineCache = new Map<string, { timeline: BranchTimeline; fetchedAt: number }>();
const inFlightTimelines = new Map<string, Promise<BranchTimeline>>();

export function invalidateBranchTimeline(branchId: string): void {
  timelineCache.delete(branchId);
  inFlightTimelines.delete(branchId);
  window.dispatchEvent(
    new CustomEvent('timeline-invalidated', { detail: { branchIds: [branchId] } })
  );
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

  const request = invokeCommand<BranchTimeline>('get_branch_timeline', { branchId })
    .then((timeline) => {
      if (inFlightTimelines.get(branchId) === request) {
        timelineCache.set(branchId, { timeline, fetchedAt: Date.now() });
      }
      return timeline;
    })
    .finally(() => {
      if (inFlightTimelines.get(branchId) === request) {
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
  }
  window.dispatchEvent(new CustomEvent('timeline-invalidated', { detail: { branchIds } }));
}

export function pullBranchFastForward(branchId: string): Promise<void> {
  return invokeCommand('pull_branch_ff_only', { branchId });
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
  sortOrder: number
): Promise<void> {
  return invokeCommand('update_project_action', {
    actionId,
    name,
    command,
    actionType,
    sortOrder,
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

export function createRepoAction(
  githubRepo: string,
  subpath: string | undefined,
  name: string,
  command: string,
  actionType: string,
  sortOrder: number
): Promise<ProjectAction> {
  return invokeCommand('create_repo_action', {
    githubRepo,
    subpath: subpath ?? null,
    name,
    command,
    actionType,
    sortOrder,
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

/** Scan the system for installed ACP-compatible agents. */
export function discoverAcpProviders(): Promise<AcpProviderInfo[]> {
  return invokeCommand('discover_acp_providers');
}

// =============================================================================
// Sessions
// =============================================================================

export function getSession(sessionId: string): Promise<Session | null> {
  return invokeCommand('get_session', { sessionId });
}

export function getSessionMessages(sessionId: string): Promise<SessionMessage[]> {
  return invokeCommand('get_session_messages', { sessionId });
}

export function getSessionMessagesSince(
  sessionId: string,
  sinceId: number
): Promise<SessionMessage[]> {
  return invokeCommand('get_session_messages_since', { sessionId, sinceId });
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
  provider?: string
): Promise<Session> {
  return invokeCommand('start_session', { prompt, workingDir, provider: provider ?? null });
}

/** Send a follow-up message to an existing session.
 *  The backend uses the provider that originally created the session. */
export function resumeSession(
  sessionId: string,
  prompt: string,
  imageIds?: string[],
  branchId?: string | null
): Promise<void> {
  return invokeCommand('resume_session', {
    sessionId,
    prompt,
    imageIds: imageIds ?? null,
    branchId: branchId ?? null,
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
  launchContext?: BranchSessionLaunchContext
): Promise<BranchSessionResponse> {
  return invokeCommand('start_branch_session', {
    branchId,
    prompt,
    sessionType,
    provider: provider ?? null,
    imageIds: imageIds ?? null,
    launchContext: launchContext ?? null,
  });
}

/** Queue a branch-scoped session to run after current work completes. */
export function queueBranchSession(
  branchId: string,
  prompt: string,
  sessionType: BranchSessionType,
  provider?: string,
  imageIds?: string[],
  launchContext?: BranchSessionLaunchContext
): Promise<BranchSessionResponse> {
  return invokeCommand('queue_branch_session', {
    branchId,
    prompt,
    sessionType,
    provider: provider ?? null,
    imageIds: imageIds ?? null,
    launchContext: launchContext ?? null,
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
): Promise<DiffFilesResponse> {
  return invokeCommand('get_diff_files', { branchId, commitSha, scope });
}

/** Get the full diff content for a single file. */
export function getFileDiff(
  branchId: string,
  commitSha: string,
  scope: DiffScope,
  path: string
): Promise<FileDiff> {
  return invokeCommand('get_file_diff', { branchId, commitSha, scope, path });
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
// Auto review
// =============================================================================

/** Find an auto review created after all commits on a branch. */
export function findFreshAutoReview(branchId: string): Promise<Review | null> {
  return invokeCommand('find_fresh_auto_review', { branchId });
}

/** Mark or unmark a review as auto-generated. */
export function setReviewAuto(reviewId: string, isAuto: boolean): Promise<void> {
  return invokeCommand('set_review_auto', { reviewId, isAuto });
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
 *  Returns the session ID so the frontend can track progress. */
export function pushBranch(branchId: string, provider?: string, force?: boolean): Promise<string> {
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
}

/** Rebase a branch via a pipeline.
 *  When target is 'base' (default), rebases onto origin/{base_branch}.
 *  When target is 'origin', rebases onto origin/{branch_name}.
 *  Returns the session ID so the frontend can track progress. */
export function rebaseBranch(
  branchId: string,
  provider?: string,
  target?: 'base' | 'origin'
): Promise<string> {
  return invokeCommand('rebase_branch', {
    branchId,
    provider: provider ?? null,
    target: target ?? null,
  });
}

/** Squash all commits on a branch into a single commit via a pipeline.
 *  Uses git reset --soft then hands off to AI to write the commit message.
 *  Returns the session ID so the frontend can track progress. */
export function squashCommits(branchId: string, provider?: string): Promise<string> {
  return invokeCommand('squash_commits', {
    branchId,
    provider: provider ?? null,
  });
}

// =============================================================================
// Doctor (Health Check)
// =============================================================================

export interface DoctorCheck {
  id: string;
  label: string;
  status: 'pass' | 'warn' | 'fail';
  message: string;
  fixUrl: string | null;
  fixCommand: string | null;
  fixType: 'command' | 'bridge' | null;
  path: string | null;
  bridgePath: string | null;
  rawOutput: string | null;
}

export interface DoctorReport {
  checks: DoctorCheck[];
}

/** Run all system health checks. */
export function runDoctor(): Promise<DoctorReport> {
  return invokeCommand('run_doctor');
}

/** Run a fix for a doctor check, identified by check ID and fix type. */
export function runDoctorFix(checkId: string, fixType: 'command' | 'bridge'): Promise<void> {
  return invokeCommand('run_doctor_fix', { checkId, fixType });
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
  repos: [string, string][]
): Promise<import('./types').RepoBadge[]> {
  return invokeCommand('ensure_repo_badges', { repos });
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
