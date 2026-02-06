import { invoke } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';

// =============================================================================
// Types
// =============================================================================

/** A git project that groups branches together with settings */
export interface GitProject {
  id: string;
  /** Path to the git repository */
  repoPath: string;
  /** Optional subpath within the repo (for monorepos) */
  subpath: string | null;
  createdAt: number;
  updatedAt: number;
}

/** Type of a project action */
export type ActionType = 'prerun' | 'run' | 'build' | 'format' | 'check' | 'test' | 'cleanUp';

/** A configurable action that can be run on a project */
export interface ProjectAction {
  id: string;
  projectId: string;
  name: string;
  command: string;
  actionType: ActionType;
  sortOrder: number;
  autoCommit: boolean;
  createdAt: number;
  updatedAt: number;
}

/** A suggested action detected from the project */
export interface SuggestedAction {
  name: string;
  command: string;
  actionType: ActionType;
  autoCommit: boolean;
  source: string;
}

/** Status of an action execution */
export type ActionStatus = 'running' | 'completed' | 'failed' | 'stopped';

/** Event for action status changes */
export interface ActionStatusEvent {
  executionId: string;
  branchId: string;
  actionId: string;
  actionName: string;
  status: ActionStatus;
  exitCode: number | null;
  startedAt: number;
  completedAt: number | null;
}

/** Event for action output */
export interface ActionOutputEvent {
  executionId: string;
  chunk: string;
  stream: 'stdout' | 'stderr';
}

/** Buffered output chunk */
export interface OutputChunk {
  chunk: string;
  stream: 'stdout' | 'stderr';
  timestamp: number;
}

/** A tracked branch with an associated worktree */
export interface Branch {
  id: string;
  /** The project this branch belongs to */
  projectId: string;
  /** Path to the original repository */
  repoPath: string;
  /** Name of the branch (e.g., "feature/auth-flow") */
  branchName: string;
  /** Path to the worktree directory */
  worktreePath: string;
  /** The branch we forked from (for computing diffs) */
  baseBranch: string;
  /** The PR number this branch was created from (if any) */
  prNumber: number | null;
  createdAt: number;
  updatedAt: number;
}

/** A git branch reference for base branch selection */
export interface BranchRef {
  /** Short name (e.g., "main", "origin/main") */
  name: string;
  /** Whether this is a remote-tracking branch */
  isRemote: boolean;
  /** The remote name if this is a remote branch (e.g., "origin") */
  remote: string | null;
}

/** Status of a branch session */
export type BranchSessionStatus = 'running' | 'completed' | 'error';

/** A session tied to a branch, producing a commit */
export interface BranchSession {
  id: string;
  branchId: string;
  /** The AI session ID (for watching/resuming) */
  aiSessionId: string | null;
  /** The commit SHA produced by this session (null while running) */
  commitSha: string | null;
  status: BranchSessionStatus;
  /** The user's prompt that started this session */
  prompt: string;
  /** Error message if status is 'error' */
  errorMessage: string | null;
  createdAt: number;
  updatedAt: number;
}

/** Commit info for display */
export interface CommitInfo {
  sha: string;
  shortSha: string;
  subject: string;
  author: string;
  timestamp: number;
}

/** Status of a branch note */
export type BranchNoteStatus = 'generating' | 'complete' | 'error';

/** A markdown note attached to a branch */
export interface BranchNote {
  id: string;
  branchId: string;
  /** The AI session ID (for viewing the generation conversation) */
  aiSessionId: string | null;
  /** Title of the note */
  title: string;
  /** Markdown content of the note */
  content: string;
  status: BranchNoteStatus;
  /** The user's prompt that started this note */
  prompt: string;
  /** Error message if status is 'error' */
  errorMessage: string | null;
  createdAt: number;
  updatedAt: number;
}

// =============================================================================
// Branch Operations
// =============================================================================

/**
 * Create a new branch with a worktree.
 * If baseBranch is not provided, uses the detected default branch.
 */
export async function createBranch(
  projectId: string,
  repoPath: string,
  branchName: string,
  baseBranch?: string
): Promise<Branch> {
  return invoke<Branch>('create_branch', { projectId, repoPath, branchName, baseBranch });
}

/**
 * Create a new branch from an existing GitHub PR.
 * Fetches the PR's head ref and creates a local branch + worktree at that commit.
 * The branch name will be the PR's head_ref (e.g., "feature-x").
 */
export async function createBranchFromPr(
  projectId: string,
  repoPath: string,
  prNumber: number,
  headRef: string,
  baseRef: string
): Promise<Branch> {
  return invoke<Branch>('create_branch_from_pr', {
    projectId,
    repoPath,
    prNumber,
    headRef,
    baseRef,
  });
}

/**
 * Get a branch by ID.
 */
export async function getBranch(branchId: string): Promise<Branch | null> {
  return invoke<Branch | null>('get_branch', { branchId });
}

/**
 * List all branches.
 */
export async function listBranches(): Promise<Branch[]> {
  return invoke<Branch[]>('list_branches');
}

/**
 * List branches for a specific repository.
 */
export async function listBranchesForRepo(repoPath: string): Promise<Branch[]> {
  return invoke<Branch[]>('list_branches_for_repo', { repoPath });
}

/**
 * List branches for a specific project.
 */
export async function listBranchesForProject(projectId: string): Promise<Branch[]> {
  return invoke<Branch[]>('list_branches_for_project', { projectId });
}

/**
 * List git branches (local and remote) for base branch selection.
 * Returns branches sorted with local first, then remote.
 */
export async function listGitBranches(repoPath: string): Promise<BranchRef[]> {
  return invoke<BranchRef[]>('list_git_branches', { repoPath });
}

/**
 * Detect the default branch for a repository.
 * Returns the remote-tracking branch (e.g., "origin/main") if available.
 */
export async function detectDefaultBranch(repoPath: string): Promise<string> {
  return invoke<string>('detect_default_branch', { repoPath });
}

/**
 * Delete a branch and its worktree.
 */
export async function deleteBranch(branchId: string): Promise<void> {
  return invoke<void>('delete_branch', { branchId });
}

/**
 * Update a branch's base branch.
 * Used to change which branch the diff is computed against.
 */
export async function updateBranchBase(branchId: string, baseBranch: string): Promise<void> {
  return invoke<void>('update_branch_base', { branchId, baseBranch });
}

// =============================================================================
// Commit Operations
// =============================================================================

/**
 * Get commits for a branch since it diverged from base.
 * Returns commits in reverse chronological order (newest first).
 */
export async function getBranchCommits(branchId: string): Promise<CommitInfo[]> {
  return invoke<CommitInfo[]>('get_branch_commits', { branchId });
}

/**
 * Get the HEAD commit SHA for a branch's worktree.
 */
export async function getBranchHead(branchId: string): Promise<string> {
  return invoke<string>('get_branch_head', { branchId });
}

/** Result of updating a branch from its associated PR */
export interface UpdateBranchFromPrResult {
  /** The commit SHA before the update */
  oldSha: string;
  /** The commit SHA after the update (new PR head) */
  newSha: string;
  /** Number of new commits pulled in */
  commitsAdded: number;
  /** Whether the branch was already up to date */
  alreadyUpToDate: boolean;
}

/**
 * Update a branch's worktree to match the latest PR head.
 *
 * Fetches the latest commits from the PR and fast-forwards (or resets) the local
 * branch to match. Works for both clean fast-forwards and force-pushed PRs.
 *
 * **Warning**: This will discard any local uncommitted changes and any local
 * commits that are not in the PR.
 *
 * Requires the branch to have an associated PR number (created via createBranchFromPr).
 */
export async function updateBranchFromPr(branchId: string): Promise<UpdateBranchFromPrResult> {
  return invoke<UpdateBranchFromPrResult>('update_branch_from_pr', { branchId });
}

// =============================================================================
// Session Operations
// =============================================================================

/**
 * List all sessions for a branch.
 */
export async function listBranchSessions(branchId: string): Promise<BranchSession[]> {
  return invoke<BranchSession[]>('list_branch_sessions', { branchId });
}

/**
 * Get the session associated with a specific commit.
 */
export async function getSessionForCommit(
  branchId: string,
  commitSha: string
): Promise<BranchSession | null> {
  return invoke<BranchSession | null>('get_session_for_commit', { branchId, commitSha });
}

/**
 * Get the currently running session for a branch (if any).
 */
export async function getRunningSession(branchId: string): Promise<BranchSession | null> {
  return invoke<BranchSession | null>('get_running_session', { branchId });
}

// =============================================================================
// Session Lifecycle
// =============================================================================

/** Response from starting a branch session */
export interface StartBranchSessionResponse {
  branchSessionId: string;
  aiSessionId: string;
}

/**
 * Start a new session on a branch.
 * The backend handles all context gathering (commits, notes, etc.) and builds the full prompt.
 *
 * @param branchId - The branch to start the session on
 * @param userPrompt - The user's task description
 * @param agentId - Optional AI agent/provider to use (e.g., "goose", "claude")
 */
export async function startBranchSession(
  branchId: string,
  userPrompt: string,
  agentId?: string
): Promise<StartBranchSessionResponse> {
  return invoke<StartBranchSessionResponse>('start_branch_session', {
    branchId,
    userPrompt,
    agentId,
  });
}

/**
 * Mark a branch session as completed with a commit SHA.
 */
export async function completeBranchSession(
  branchSessionId: string,
  commitSha: string
): Promise<void> {
  return invoke<void>('complete_branch_session', { branchSessionId, commitSha });
}

/**
 * Mark a branch session as failed with an error message.
 */
export async function failBranchSession(
  branchSessionId: string,
  errorMessage: string
): Promise<void> {
  return invoke<void>('fail_branch_session', { branchSessionId, errorMessage });
}

/**
 * Cancel a running branch session (deletes the record).
 * Used to recover from stuck sessions.
 */
export async function cancelBranchSession(branchSessionId: string): Promise<void> {
  return invoke<void>('cancel_branch_session', { branchSessionId });
}

/**
 * Delete a branch session and its associated commit.
 * This will also delete all commits that came after this one (resets to parent).
 * Returns the number of commits that were removed.
 */
export async function deleteBranchSessionAndCommit(branchSessionId: string): Promise<number> {
  return invoke<number>('delete_branch_session_and_commit', { branchSessionId });
}

/**
 * Check if a branch session's AI session is actually alive.
 * Returns true if the session has a live connection,
 * false if the session is dead (no live connection).
 */
export async function isSessionAlive(aiSessionId: string): Promise<boolean> {
  return invoke<boolean>('is_session_alive', { aiSessionId });
}

/**
 * Cancel an active AI session by killing the agent subprocess.
 * This immediately terminates the running agent process.
 */
export async function cancelAiSession(aiSessionId: string): Promise<void> {
  return invoke<void>('cancel_ai_session', { aiSessionId });
}

/**
 * Restart a stuck branch session.
 * Deletes the old session and starts a new one with the same prompt.
 * Returns the new session IDs.
 */
export async function restartBranchSession(
  branchSessionId: string,
  fullPrompt: string
): Promise<StartBranchSessionResponse> {
  return invoke<StartBranchSessionResponse>('restart_branch_session', {
    branchSessionId,
    fullPrompt,
  });
}

/**
 * Recover orphaned sessions for a branch.
 * If there's a "running" session but no live AI session, checks if commits were made
 * and marks the session as completed or errored accordingly.
 * Returns the updated session if one was recovered, null otherwise.
 */
export async function recoverOrphanedSession(branchId: string): Promise<BranchSession | null> {
  return invoke<BranchSession | null>('recover_orphaned_session', { branchId });
}

/**
 * Get a branch session by its AI session ID.
 * Used to look up branch sessions when AI session status changes.
 */
export async function getBranchSessionByAiSession(
  aiSessionId: string
): Promise<BranchSession | null> {
  return invoke<BranchSession | null>('get_branch_session_by_ai_session', { aiSessionId });
}

// =============================================================================
// Note Operations
// =============================================================================

/** Response from starting a branch note */
export interface StartBranchNoteResponse {
  branchNoteId: string;
  aiSessionId: string;
}

/**
 * Start generating a new note on a branch.
 * The backend handles all context gathering (commits, notes, etc.) and builds the full prompt.
 *
 * @param branchId - The branch to create the note on
 * @param title - The title for the note
 * @param description - What the note should cover (user's description)
 * @param agentId - Optional AI agent/provider to use (e.g., "goose", "claude")
 */
export async function startBranchNote(
  branchId: string,
  title: string,
  description: string,
  agentId?: string
): Promise<StartBranchNoteResponse> {
  return invoke<StartBranchNoteResponse>('start_branch_note', {
    branchId,
    title,
    description,
    agentId,
  });
}

/**
 * List all notes for a branch.
 */
export async function listBranchNotes(branchId: string): Promise<BranchNote[]> {
  return invoke<BranchNote[]>('list_branch_notes', { branchId });
}

/**
 * Get a branch note by ID.
 */
export async function getBranchNote(noteId: string): Promise<BranchNote | null> {
  return invoke<BranchNote | null>('get_branch_note', { noteId });
}

/**
 * Get the currently generating note for a branch (if any).
 */
export async function getGeneratingNote(branchId: string): Promise<BranchNote | null> {
  return invoke<BranchNote | null>('get_generating_note', { branchId });
}

/**
 * Get a branch note by its AI session ID.
 */
export async function getBranchNoteByAiSession(aiSessionId: string): Promise<BranchNote | null> {
  return invoke<BranchNote | null>('get_branch_note_by_ai_session', { aiSessionId });
}

/**
 * Mark a branch note as completed with content.
 */
export async function completeBranchNote(noteId: string, content: string): Promise<void> {
  return invoke<void>('complete_branch_note', { noteId, content });
}

/**
 * Mark a branch note as failed with an error message.
 */
export async function failBranchNote(noteId: string, errorMessage: string): Promise<void> {
  return invoke<void>('fail_branch_note', { noteId, errorMessage });
}

/**
 * Delete a branch note.
 */
export async function deleteBranchNote(noteId: string): Promise<void> {
  return invoke<void>('delete_branch_note', { noteId });
}

/**
 * Recover an orphaned note for a branch.
 * If there's a "generating" note but the AI session is idle, extracts the final
 * message content and marks the note as complete.
 */
export async function recoverOrphanedNote(branchId: string): Promise<BranchNote | null> {
  return invoke<BranchNote | null>('recover_orphaned_note', { branchId });
}

// =============================================================================
// Open In... Operations
// =============================================================================

/** An application that can open a directory */
export interface OpenerApp {
  /** Unique identifier (e.g., "vscode", "terminal", "warp") */
  id: string;
  /** Display name (e.g., "VS Code", "Terminal") */
  name: string;
}

/**
 * Get the list of supported apps that are currently installed.
 * Results are cached for the lifetime of the app.
 */
let cachedOpeners: OpenerApp[] | null = null;

export async function getAvailableOpeners(): Promise<OpenerApp[]> {
  if (cachedOpeners !== null) return cachedOpeners;
  cachedOpeners = await invoke<OpenerApp[]>('get_available_openers');
  return cachedOpeners;
}

/**
 * Open a directory path in a specific application.
 */
export async function openInApp(path: string, appId: string): Promise<void> {
  return invoke<void>('open_in_app', { path, appId });
}

/**
 * Copy a path to the clipboard.
 */
export async function copyPathToClipboard(path: string): Promise<void> {
  await writeText(path);
}

// =============================================================================
// Pull Request Operations
// =============================================================================

/** Result of creating a pull request */
export interface CreatePrResult {
  /** The PR number */
  number: number;
  /** URL to the PR on GitHub */
  url: string;
}

/** Extended PR info including body and state */
export interface PullRequestInfo {
  number: number;
  title: string;
  body: string;
  author: string;
  baseRef: string;
  headRef: string;
  draft: boolean;
  state: string;
  url: string;
}

/**
 * Get the PR associated with a branch (if one exists).
 * Returns null if no PR exists for this branch.
 */
export async function getPrForBranch(
  repoPath: string,
  branch: string
): Promise<PullRequestInfo | null> {
  return invoke<PullRequestInfo | null>('get_pr_for_branch', { repoPath, branch });
}

/**
 * Push a branch to the remote.
 * If force is true, uses --force-with-lease for safer force pushing.
 */
export async function pushBranch(
  repoPath: string,
  branch: string,
  force: boolean = false
): Promise<void> {
  return invoke<void>('push_branch', { repoPath, branch, force });
}

/**
 * Create a new pull request on GitHub.
 * The branch must be pushed to the remote first.
 */
export async function createPullRequest(
  repoPath: string,
  headBranch: string,
  baseBranch: string,
  title: string,
  body: string,
  draft: boolean = false
): Promise<CreatePrResult> {
  return invoke<CreatePrResult>('create_pull_request', {
    repoPath,
    headBranch,
    baseBranch,
    title,
    body,
    draft,
  });
}

/**
 * Update an existing pull request's title and/or body.
 */
export async function updatePullRequest(
  repoPath: string,
  prNumber: number,
  title?: string,
  body?: string
): Promise<void> {
  return invoke<void>('update_pull_request', { repoPath, prNumber, title, body });
}

/** AI-generated PR description */
export interface GeneratedPrDescription {
  title: string;
  body: string;
}

/**
 * Generate a PR description using AI based on commits and diff.
 * Requires an AI agent (Goose) to be available.
 */
export async function generatePrDescription(
  repoPath: string,
  headBranch: string,
  baseBranch: string
): Promise<GeneratedPrDescription> {
  return invoke<GeneratedPrDescription>('generate_pr_description', {
    repoPath,
    headBranch,
    baseBranch,
  });
}

// =============================================================================
// Git Project Operations
// =============================================================================

/**
 * Create a new git project.
 * If a project already exists for the repo_path, returns an error.
 */
export async function createGitProject(repoPath: string, subpath?: string): Promise<GitProject> {
  return invoke<GitProject>('create_git_project', { repoPath, subpath });
}

/**
 * Get a git project by ID.
 */
export async function getGitProject(projectId: string): Promise<GitProject | null> {
  return invoke<GitProject | null>('get_git_project', { projectId });
}

/**
 * Get a git project by repo path.
 */
export async function getGitProjectByRepo(repoPath: string): Promise<GitProject | null> {
  return invoke<GitProject | null>('get_git_project_by_repo', { repoPath });
}

/**
 * List all git projects.
 */
export async function listGitProjects(): Promise<GitProject[]> {
  return invoke<GitProject[]>('list_git_projects');
}

/**
 * Update a git project's subpath.
 * Pass null for subpath to clear it.
 */
export async function updateGitProject(projectId: string, subpath: string | null): Promise<void> {
  return invoke<void>('update_git_project', { projectId, subpath });
}

/**
 * Delete a git project.
 * Note: This does NOT delete associated branches - they still work via repo_path.
 */
export async function deleteGitProject(projectId: string): Promise<void> {
  return invoke<void>('delete_git_project', { projectId });
}

/**
 * Get or create a git project for a repo path.
 * If no project exists, creates one with the repo folder name as the project name.
 */
export async function getOrCreateGitProject(repoPath: string): Promise<GitProject> {
  return invoke<GitProject>('get_or_create_git_project', { repoPath });
}

// =============================================================================
// Project Actions
// =============================================================================

/** List all actions for a project */
export async function listProjectActions(projectId: string): Promise<ProjectAction[]> {
  return invoke<ProjectAction[]>('list_project_actions', { projectId });
}

/** Create a new project action */
export async function createProjectAction(
  projectId: string,
  name: string,
  command: string,
  actionType: ActionType,
  sortOrder: number,
  autoCommit: boolean
): Promise<ProjectAction> {
  return invoke<ProjectAction>('create_project_action', {
    projectId,
    name,
    command,
    actionType,
    sortOrder,
    autoCommit,
  });
}

/** Update a project action */
export async function updateProjectAction(
  actionId: string,
  name: string,
  command: string,
  actionType: ActionType,
  sortOrder: number,
  autoCommit: boolean
): Promise<void> {
  return invoke<void>('update_project_action', {
    actionId,
    name,
    command,
    actionType,
    sortOrder,
    autoCommit,
  });
}

/** Delete a project action */
export async function deleteProjectAction(actionId: string): Promise<void> {
  return invoke<void>('delete_project_action', { actionId });
}

/** Reorder project actions */
export async function reorderProjectActions(actionIds: string[]): Promise<void> {
  return invoke<void>('reorder_project_actions', { actionIds });
}

/** Detect actions for a project */
export async function detectProjectActions(projectId: string): Promise<SuggestedAction[]> {
  return invoke<SuggestedAction[]>('detect_project_actions', { projectId });
}

/** Run an action on a branch */
export async function runBranchAction(branchId: string, actionId: string): Promise<string> {
  return invoke<string>('run_branch_action', { branchId, actionId });
}

/** Run prerun actions for a branch */
export async function runPrerunActions(branchId: string): Promise<void> {
  return invoke<void>('run_prerun_actions', { branchId });
}

/** Stop a running action */
export async function stopBranchAction(executionId: string): Promise<void> {
  return invoke<void>('stop_branch_action', { executionId });
}

/** Get all running actions for a branch */
export async function getRunningBranchActions(branchId: string): Promise<ActionStatusEvent[]> {
  return invoke<ActionStatusEvent[]>('get_running_branch_actions', { branchId });
}

/** Get buffered output for an execution */
export async function getActionOutputBuffer(executionId: string): Promise<OutputChunk[]> {
  return invoke<OutputChunk[]>('get_action_output_buffer', { executionId });
}
