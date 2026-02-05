import { invoke } from '@tauri-apps/api/core';

// =============================================================================
// Types
// =============================================================================

/** A tracked branch with an associated worktree */
export interface Branch {
  id: string;
  /** Path to the original repository */
  repoPath: string;
  /** Name of the branch (e.g., "feature/auth-flow") */
  branchName: string;
  /** Path to the worktree directory */
  worktreePath: string;
  /** The branch we forked from (for computing diffs) */
  baseBranch: string;
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
  repoPath: string,
  branchName: string,
  baseBranch?: string
): Promise<Branch> {
  return invoke<Branch>('create_branch', { repoPath, branchName, baseBranch });
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
 * Creates a branch_session record, starts an AI session, and sends the prompt.
 *
 * @param branchId - The branch to start the session on
 * @param userPrompt - The user's original prompt (stored for display)
 * @param fullPrompt - The full prompt with context to send to the AI
 */
export async function startBranchSession(
  branchId: string,
  userPrompt: string,
  fullPrompt: string
): Promise<StartBranchSessionResponse> {
  return invoke<StartBranchSessionResponse>('start_branch_session', {
    branchId,
    userPrompt,
    fullPrompt,
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
 * Creates an AI session, a branch_note record, and sends the prompt.
 */
export async function startBranchNote(
  branchId: string,
  title: string,
  prompt: string
): Promise<StartBranchNoteResponse> {
  return invoke<StartBranchNoteResponse>('start_branch_note', { branchId, title, prompt });
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
