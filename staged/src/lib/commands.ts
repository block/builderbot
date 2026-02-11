/**
 * Typed invoke wrappers for Tauri commands.
 *
 * One function per command. Each returns a typed promise.
 */

import { invoke } from '@tauri-apps/api/core';
import type {
  Project,
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

export function createProject(repoPath: string, subpath?: string): Promise<Project> {
  return invoke('create_project', { repoPath, subpath });
}

export function deleteProject(id: string): Promise<void> {
  return invoke('delete_project', { id });
}

// =============================================================================
// Branches
// =============================================================================

export function listBranchesForProject(projectId: string): Promise<Branch[]> {
  return invoke('list_branches_for_project', { projectId });
}

export function createBranch(
  projectId: string,
  branchName: string,
  baseBranch?: string
): Promise<Branch> {
  return invoke('create_branch', { projectId, branchName, baseBranch });
}

/** Create a remote branch backed by a Blox workspace. */
export function createRemoteBranch(
  projectId: string,
  branchName: string,
  workspaceName: string,
  baseBranch?: string,
  agent?: string,
  source?: string
): Promise<Branch> {
  return invoke('create_remote_branch', {
    projectId,
    branchName,
    baseBranch,
    workspaceName,
    agent,
    source,
  });
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

/** Send a prompt to a remote branch's running workspace agent. Returns the agent's response. */
export function sendWorkspacePrompt(branchId: string, prompt: string): Promise<string> {
  return invoke('send_workspace_prompt', { branchId, prompt });
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

export function listGitBranches(repoPath: string): Promise<BranchRef[]> {
  return invoke('list_git_branches', { repoPath });
}

export function detectDefaultBranch(repoPath: string): Promise<string> {
  return invoke('detect_default_branch_cmd', { repoPath });
}
