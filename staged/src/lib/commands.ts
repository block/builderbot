/**
 * Typed invoke wrappers for Tauri commands.
 *
 * One function per command. Each returns a typed promise.
 */

import { invoke } from '@tauri-apps/api/core';
import type {
  Project,
  ProjectRepo,
  GitHubRepo,
  Branch,
  BranchTimeline,
  BranchRef,
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
} from './types';

// =============================================================================
// Store status
// =============================================================================

/** Returns null if the store is ready, or version info if a reset is needed. */
export function getStoreStatus(): Promise<StoreIncompatibility | null> {
  return invoke('get_store_status');
}

/** Delete the old database and create a fresh store. Called after user confirms. */
export function confirmResetStore(): Promise<void> {
  return invoke('confirm_reset_store');
}

// =============================================================================
// Projects
// =============================================================================

export function listProjects(): Promise<Project[]> {
  return invoke('list_projects');
}

export function createProject(name: string, githubRepo?: string, subpath?: string): Promise<Project> {
  return invoke('create_project', { name, githubRepo: githubRepo ?? null, subpath });
}

export function deleteProject(id: string): Promise<void> {
  return invoke('delete_project', { id });
}

export function listProjectRepos(projectId: string): Promise<ProjectRepo[]> {
  return invoke('list_project_repos', { projectId });
}

export function addProjectRepo(
  projectId: string,
  githubRepo: string,
  subpath?: string,
  setAsPrimary?: boolean
): Promise<ProjectRepo> {
  return invoke('add_project_repo', {
    projectId,
    githubRepo,
    subpath: subpath ?? null,
    setAsPrimary: setAsPrimary ?? null,
  });
}

export function removeProjectRepo(projectId: string, projectRepoId: string): Promise<void> {
  return invoke('remove_project_repo', { projectId, projectRepoId });
}

export function setPrimaryProjectRepo(projectId: string, projectRepoId: string): Promise<void> {
  return invoke('set_primary_project_repo', { projectId, projectRepoId });
}

/** List the authenticated user's GitHub organization memberships. */
export function listGithubOrgs(): Promise<string[]> {
  return invoke('list_github_orgs');
}

/** List GitHub repositories for the authenticated user or a specific owner. */
export function listGithubRepos(owner?: string): Promise<GitHubRepo[]> {
  return invoke('list_github_repos', { owner: owner ?? null });
}

/** List repositories the authenticated user has recently pushed to.
 *  Returns repos across all orgs, sorted by most recently pushed. */
export function listUserRepos(limit?: number): Promise<GitHubRepo[]> {
  return invoke('list_user_repos', { limit: limit ?? null });
}

/** Fetch a single GitHub repository by owner/repo.
 *  Returns null if the repo doesn't exist or user lacks access. */
export function getGithubRepo(owner: string, repo: string): Promise<GitHubRepo | null> {
  return invoke('get_github_repo', { owner, repo });
}

/** Search GitHub repositories for the authenticated user or a specific owner. */
export function searchGithubRepos(query: string, owner?: string): Promise<GitHubRepo[]> {
  return invoke('search_github_repos', { query, owner: owner ?? null });
}

// =============================================================================
// Branches
// =============================================================================

export function listBranchesForProject(projectId: string): Promise<Branch[]> {
  return invoke('list_branches_for_project', { projectId });
}

/** Create a local branch record (DB only — no git worktree yet).
 *  Returns immediately with worktreePath = null.
 *  Call `setupWorktree` separately to create the git worktree. */
export function createBranch(
  projectId: string,
  branchName: string,
  baseBranch?: string
): Promise<Branch> {
  return invoke('create_branch', { projectId, branchName, baseBranch });
}

/** Create the git worktree for a local branch and record its workdir.
 *  Returns the updated branch with worktreePath populated. */
export function setupWorktree(branchId: string): Promise<Branch> {
  return invoke('setup_worktree', { branchId });
}

/** Import a GitHub PR: fetch its head ref, create a local branch + worktree,
 *  and record everything in the DB. Returns the branch with worktreePath
 *  already populated (no separate setupWorktree call needed). */
export function setupWorktreeFromPr(
  projectId: string,
  prNumber: number,
  headRef: string,
  baseRef: string
): Promise<Branch> {
  return invoke('setup_worktree_from_pr', { projectId, prNumber, headRef, baseRef });
}

/** Create a remote branch record (does not start the workspace). */
export function createRemoteBranch(
  projectId: string,
  branchName: string,
  workspaceName: string,
  baseBranch?: string
): Promise<Branch> {
  return invoke('create_remote_branch', {
    projectId,
    branchName,
    baseBranch,
    workspaceName,
  });
}

/** Start the Blox workspace for a remote branch. */
export function startWorkspace(branchId: string): Promise<void> {
  return invoke('start_workspace', { branchId });
}

export function deleteBranch(branchId: string): Promise<void> {
  return invoke('delete_branch', { branchId });
}

/** Get info about a remote branch's Blox workspace. */
export function getWorkspaceInfo(branchId: string): Promise<WorkspaceInfo> {
  return invoke('get_workspace_info', { branchId });
}

/** Poll a remote branch's workspace status, update the DB, and return the new status string. */
export function pollWorkspaceStatus(branchId: string): Promise<string> {
  return invoke('poll_workspace_status', { branchId });
}

// =============================================================================
// Timeline
// =============================================================================

export function getBranchTimeline(branchId: string): Promise<BranchTimeline> {
  return invoke('get_branch_timeline', { branchId });
}

// =============================================================================
// Actions
// =============================================================================

export interface ProjectAction {
  id: string;
  projectId: string;
  name: string;
  command: string;
  actionType: string;
  sortOrder: number;
  autoCommit: boolean;
  createdAt: number;
  updatedAt: number;
}

export function listProjectActions(projectId: string): Promise<ProjectAction[]> {
  return invoke('list_project_actions', { projectId });
}

export function createProjectAction(
  projectId: string,
  name: string,
  command: string,
  actionType: string,
  sortOrder: number,
  autoCommit: boolean
): Promise<ProjectAction> {
  return invoke('create_project_action', {
    projectId,
    name,
    command,
    actionType,
    sortOrder,
    autoCommit,
  });
}

export function updateProjectAction(
  actionId: string,
  name: string,
  command: string,
  actionType: string,
  sortOrder: number,
  autoCommit: boolean
): Promise<void> {
  return invoke('update_project_action', {
    actionId,
    name,
    command,
    actionType,
    sortOrder,
    autoCommit,
  });
}

export function deleteProjectAction(actionId: string): Promise<void> {
  return invoke('delete_project_action', { actionId });
}

// =============================================================================
// Utilities
// =============================================================================

/** Open a URL in the user's default browser. */
export function openUrl(url: string): Promise<void> {
  return invoke('open_url', { url });
}

/** Read a text file from an absolute path (used for Tauri native drag-and-drop). */
export function readTextFile(filePath: string): Promise<string> {
  return invoke('read_text_file', { filePath });
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
  return invoke('discover_acp_providers');
}

// =============================================================================
// Sessions
// =============================================================================

export function getSession(sessionId: string): Promise<Session | null> {
  return invoke('get_session', { sessionId });
}

export function getSessionMessages(sessionId: string): Promise<SessionMessage[]> {
  return invoke('get_session_messages', { sessionId });
}

export function getSessionMessagesSince(
  sessionId: string,
  sinceId: number
): Promise<SessionMessage[]> {
  return invoke('get_session_messages_since', { sessionId, sinceId });
}

/** Create a session and immediately start the agent. */
export function startSession(
  prompt: string,
  workingDir: string,
  provider?: string
): Promise<Session> {
  return invoke('start_session', { prompt, workingDir, provider: provider ?? null });
}

/** Send a follow-up message to an existing session.
 *  The backend uses the provider that originally created the session. */
export function resumeSession(sessionId: string, prompt: string): Promise<void> {
  return invoke('resume_session', { sessionId, prompt });
}

export function cancelSession(sessionId: string): Promise<void> {
  return invoke('cancel_session', { sessionId });
}

export function deleteSession(sessionId: string): Promise<void> {
  return invoke('delete_session', { sessionId });
}

/** Start a branch-scoped session (note or commit). */
export function startBranchSession(
  branchId: string,
  prompt: string,
  sessionType: BranchSessionType,
  provider?: string
): Promise<BranchSessionResponse> {
  return invoke('start_branch_session', {
    branchId,
    prompt,
    sessionType,
    provider: provider ?? null,
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
  return invoke('create_note', { branchId, title, content });
}

/** Delete a note and optionally its linked session. */
export function deleteNote(noteId: string, deleteSession = true): Promise<void> {
  return invoke('delete_note', { noteId, deleteSession });
}

/** Delete a commit (git reset --hard to parent) and optionally its session.
 *  Only works for the tip commit (HEAD) of the branch. */
export function deleteCommit(
  branchId: string,
  commitSha: string,
  deleteSession = true
): Promise<void> {
  return invoke('delete_commit', { branchId, commitSha, deleteSession });
}

/** Delete a pending commit (no SHA) by its DB id, optionally its session. */
export function deletePendingCommit(commitId: string, deleteSession = true): Promise<void> {
  return invoke('delete_pending_commit', { commitId, deleteSession });
}

/** Delete a review and all its comments, optionally its linked session. */
export function deleteReview(reviewId: string, deleteSession = true): Promise<void> {
  return invoke('delete_review', { reviewId, deleteSession });
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
  scope: 'branch' | 'commit' = 'branch'
): Promise<DiffFilesResponse> {
  return invoke('get_diff_files', { branchId, commitSha, scope });
}

/** Get the full diff content for a single file. */
export function getFileDiff(
  branchId: string,
  commitSha: string,
  scope: 'branch' | 'commit',
  path: string
): Promise<FileDiff> {
  return invoke('get_file_diff', { branchId, commitSha, scope, path });
}

/** Get file content at a specific ref (for reference files). */
export function getFileAtRef(branchId: string, refName: string, path: string): Promise<File> {
  return invoke('get_file_at_ref', { branchId, refName, path });
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
  return invoke('ensure_review', { branchId, commitSha, scope });
}

/** Find an existing review by (branch, commit, scope) without creating one. */
export function findReview(
  branchId: string,
  commitSha: string,
  scope: 'branch' | 'commit'
): Promise<Review | null> {
  return invoke('find_review', { branchId, commitSha, scope });
}

/** Get a review by ID with all child data. */
export function getReview(reviewId: string): Promise<Review | null> {
  return invoke('get_review', { reviewId });
}

/** Mark a file as reviewed. */
export function markReviewed(reviewId: string, path: string): Promise<void> {
  return invoke('mark_reviewed', { reviewId, path });
}

/** Unmark a file as reviewed. */
export function unmarkReviewed(reviewId: string, path: string): Promise<void> {
  return invoke('unmark_reviewed', { reviewId, path });
}

/** Add a comment to a review. */
export function addComment(
  reviewId: string,
  path: string,
  spanStart: number,
  spanEnd: number,
  content: string
): Promise<Comment> {
  return invoke('add_comment', { reviewId, path, spanStart, spanEnd, content });
}

/** Update a comment's content. */
export function updateComment(commentId: string, content: string): Promise<void> {
  return invoke('update_comment', { commentId, content });
}

/** Delete a comment. */
export function deleteComment(commentId: string): Promise<void> {
  return invoke('delete_comment', { commentId });
}

/** Add a reference file to a review. */
export function addReferenceFile(reviewId: string, path: string): Promise<void> {
  return invoke('add_reference_file', { reviewId, path });
}

/** Remove a reference file from a review. */
export function removeReferenceFile(reviewId: string, path: string): Promise<void> {
  return invoke('remove_reference_file', { reviewId, path });
}

// =============================================================================
// Git helpers
// =============================================================================

/** Check whether the `sq` CLI is available on this system. */
export function isSqAvailable(): Promise<boolean> {
  return invoke('is_sq_available');
}

/** Check whether the user is authenticated with Blox.
 *  Resolves if authenticated, rejects with an error message if not. */
export function checkBloxAuth(): Promise<void> {
  return invoke('check_blox_auth');
}

export function listGitBranches(githubRepo: string): Promise<BranchRef[]> {
  return invoke('list_git_branches', { githubRepo });
}

export function detectDefaultBranch(githubRepo: string): Promise<string> {
  return invoke('detect_default_branch_cmd', { githubRepo });
}

/** Prune stale remote-tracking refs in the background.
 *  With GitHub-repo-based projects, this is a no-op. */
export function pruneRemoteRefs(githubRepo: string): Promise<void> {
  return invoke('prune_remote_refs', { githubRepo });
}

/** Check whether a branch already exists locally for this project. */
export function checkExistingLocalBranch(projectId: string, branchName: string): Promise<boolean> {
  return invoke('check_existing_local_branch', { projectId, branchName });
}

export function listPullRequests(githubRepo: string): Promise<PullRequest[]> {
  return invoke('list_pull_requests', { githubRepo });
}

export function listIssues(githubRepo: string): Promise<Issue[]> {
  return invoke('list_issues', { githubRepo });
}

// =============================================================================
// PR creation
// =============================================================================

/** Kick off an agent session to push the branch and create a PR via `gh`.
 *  Returns the session ID so the frontend can track progress. */
export function createPr(branchId: string, provider?: string): Promise<string> {
  return invoke('create_pr', { branchId, provider: provider ?? null });
}

/** Build the GitHub PR URL from the repo's origin remote and a PR number. */
export function getPrUrl(branchId: string, prNumber: number): Promise<string> {
  return invoke('get_pr_url', { branchId, prNumber });
}

/** Update the PR number stored for a branch. */
export function updateBranchPr(branchId: string, prNumber: number | null): Promise<void> {
  return invoke('update_branch_pr', { branchId, prNumber });
}

/** Check whether a branch has local commits not yet pushed to the remote. */
export function hasUnpushedCommits(branchId: string): Promise<boolean> {
  return invoke('has_unpushed_commits', { branchId });
}

/** Push a branch to its remote via an agent session.
 *  The agent runs git push and can fix pre-push hook failures.
 *  Returns the session ID so the frontend can track progress. */
export function pushBranch(branchId: string, provider?: string, force?: boolean): Promise<string> {
  return invoke('push_branch', {
    branchId,
    provider: provider ?? null,
    force: force ?? null,
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
}

export interface DoctorReport {
  checks: DoctorCheck[];
}

/** Run all system health checks. */
export function runDoctor(): Promise<DoctorReport> {
  return invoke('run_doctor');
}

/** Run a fix command from a doctor check. */
export function runDoctorFix(command: string): Promise<void> {
  return invoke('run_doctor_fix', { command });
}
