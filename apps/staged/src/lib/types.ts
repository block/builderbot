/**
 * Frontend types matching backend serde output.
 *
 * All types use camelCase (matching the backend's #[serde(rename_all = "camelCase")]).
 */

export interface Project {
  id: string;
  name: string;
  githubRepo: string | null;
  location: ProjectLocation;
  subpath: string | null;
  createdAt: number;
  updatedAt: number;
}

export type ProjectLocation = 'local' | 'remote';

export interface ProjectRepo {
  id: string;
  projectId: string;
  githubRepo: string;
  branchName: string;
  subpath: string | null;
  isPrimary: boolean;
  reason: string | null;
  /** For fork PRs, the head (fork) repo slug. Null for non-fork PRs. */
  headRepo: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface RecentRepo {
  id: string;
  githubRepo: string;
  subpath: string | null;
  lastUsedAt: number;
}

export interface SuggestedRepo {
  githubRepo: string;
  subpath: string | null;
  score: number;
}

export interface GitHubRepo {
  name: string;
  nameWithOwner: string;
  description: string | null;
  isPrivate: boolean;
  updatedAt: string;
}

export type BranchType = 'local' | 'remote';
export type WorkspaceStatus = 'starting' | 'running' | 'stopped' | 'suspended' | 'error';

export interface Branch {
  id: string;
  projectId: string;
  projectRepoId: string | null;
  branchName: string;
  baseBranch: string;
  prNumber: number | null;
  branchType: BranchType;
  workspaceName: string | null;
  workstationId: number | null;
  workspaceStatus: WorkspaceStatus | null;
  setupComplete: boolean;
  worktreePath: string | null;
  createdAt: number;
  updatedAt: number;
  // PR status fields
  prState: string | null; // "OPEN", "CLOSED", "MERGED"
  prChecksStatus: string | null; // "SUCCESS", "FAILURE", "PENDING", "EXPECTED"
  prReviewDecision: string | null; // "APPROVED", "CHANGES_REQUESTED", "REVIEW_REQUIRED"
  prMergeable: boolean | null;
  prDraft: boolean | null;
  prUrl: string | null;
  prUpdatedAt: number | null;
  prFetchedAt: number | null;
  prHeadSha: string | null;
  /** Number of finalized commits on this branch (populated by list_branches_for_project). */
  commitCount?: number;
}

// PR status types
export interface PrStatus {
  state: string; // "OPEN", "CLOSED", "MERGED"
  isDraft: boolean;
  mergeable: string;
  reviewDecision: string | null;
  checksSummary: {
    total: number;
    passed: number;
    failed: number;
    pending: number;
    state: string; // "SUCCESS", "FAILURE", "PENDING", "EXPECTED"
  };
  headSha: string | null;
  failedChecks: PrFailedCheck[];
}

export interface PrFailedCheck {
  name: string;
  state: string;
  detailsUrl: string | null;
}

export interface PrStatusChangedEvent {
  branchId: string;
  prState: string;
  prChecksStatus: string;
  prReviewDecision: string | null;
  prMergeable: boolean;
  prDraft: boolean;
  prHeadSha: string | null;
  prFetchedAt: number | null;
  failedChecks: PrFailedCheck[];
}

export interface CommitTimelineItem {
  /** DB id — present for pending/failed commits so they can be deleted by id. */
  id: string | null;
  sha: string;
  shortSha: string;
  subject: string;
  author: string;
  authorEmail: string;
  /** Unix timestamp in seconds */
  timestamp: number;
  /** Position in git's topological order (0 = oldest). Tiebreaker for same-second timestamps. */
  order: number;
  sessionId: string | null;
  sessionStatus: string | null;
  completionReason: string | null;
  /** Whether this commit was authored by the current git user. */
  isOwnCommit: boolean;
}

export interface NoteTimelineItem {
  id: string;
  title: string;
  content: string;
  sessionId: string | null;
  sessionStatus: string | null;
  completionReason: string | null;
  createdAt: number;
  updatedAt: number;
  completedAt: number | null;
  suggestedNextCommitStep: string | null;
  suggestedNextNoteStep: string | null;
}

export interface ReviewTimelineItem {
  id: string;
  commitSha: string;
  scope: string;
  sessionId: string | null;
  sessionStatus: string | null;
  sessionProvider: string | null;
  completionReason: string | null;
  title: string | null;
  commentCount: number;
  isAuto: boolean;
  createdAt: number;
  updatedAt: number;
  completedAt: number | null;
}

export interface ImageTimelineItem {
  id: string;
  filename: string;
  mimeType: string;
  sizeBytes: number;
  sessionId: string | null;
  sessionStatus: string | null;
  completionReason: string | null;
  createdAt: number;
}

export interface BranchTimeline {
  commits: CommitTimelineItem[];
  notes: NoteTimelineItem[];
  reviews: ReviewTimelineItem[];
  images: ImageTimelineItem[];
  gitState?: BranchGitState | null;
}

export type UpstreamRelation = 'missing' | 'inSync' | 'localAhead' | 'originAhead' | 'diverged';

export type FetchStatus = 'fresh' | 'stale' | 'failed';

export interface BranchGitState {
  headSha: string | null;
  currentBranch: string | null;
  detachedHead: boolean;
  expectedBranchMatches: boolean;
  upstream: {
    ref: string;
    exists: boolean;
    sha: string | null;
    relation: UpstreamRelation;
    ahead: number;
    behind: number;
    mergeBaseSha: string | null;
    /** Number of commits `origin/{base}` is ahead of `origin/{branch}`. */
    behindBase: number;
  };
  base: {
    ref: string;
    sha: string | null;
    commitsSinceFork: number;
  };
  worktree: {
    dirty: boolean;
    modified: number;
    added: number;
    deleted: number;
    untracked: number;
    conflicted: number;
  };
  fetch: {
    status: FetchStatus;
    fetchedAt: number | null;
    error: string | null;
  };
}

export interface Image {
  id: string;
  branchId: string | null;
  projectId: string;
  sessionId: string | null;
  filename: string;
  mimeType: string;
  sizeBytes: number;
  createdAt: number;
}

export interface BranchRef {
  name: string;
  isRemote: boolean;
  remote: string | null;
}

export interface RepoBadge {
  githubRepo: string;
  subpath: string;
  shortName: string;
  hue: number;
  createdAt: number;
  pinned: boolean;
  pinSortOrder: number | null;
  defaultBranch: string | null;
}

/** A repo badge enriched with clone-state for the home screen. */
export interface RepoHomeItem extends RepoBadge {
  /** Whether this repo has a local clone on disk. */
  hasLocalClone: boolean;
}

/** Timeline of commits on a repo's default branch. */
export interface RepoDefaultBranchTimeline {
  commits: CommitTimelineItem[];
  defaultBranch: string;
}

// =============================================================================
// GitHub types (for PR/Issue picker)
// =============================================================================

export interface PullRequest {
  number: number;
  title: string;
  body: string;
  author: string;
  baseRef: string;
  headRef: string;
  /** The repository the PR's head branch lives in (e.g. "fork-owner/repo" for fork PRs). */
  headRepo: string | null;
  draft: boolean;
  updatedAt: string;
}

export interface Issue {
  number: number;
  title: string;
  body: string;
  author: string;
  updatedAt: string;
  labels: string[];
}

// =============================================================================
// Project notes & sessions
// =============================================================================

export interface ProjectNote {
  id: string;
  projectId: string;
  sessionId: string | null;
  title: string;
  content: string;
  createdAt: number;
  updatedAt: number;
  completedAt: number | null;
  suggestedNextCommitStep: string | null;
  suggestedNextNoteStep: string | null;
  sessionStatus: string | null;
  completionReason: string | null;
}

export interface ProjectSessionResponse {
  sessionId: string;
  noteId: string;
}

// =============================================================================
// Sessions
// =============================================================================

export type SessionStatus = 'queued' | 'running' | 'completed' | 'error' | 'cancelled';

export type CompletionReason =
  | 'turn_complete'
  | 'interrupted'
  | 'project_session_interrupted'
  | 'crashed'
  | 'app_quit'
  | 'unknown';

/** Completion reasons that indicate a session can be resumed. */
export const RESUMABLE_REASONS: ReadonlySet<CompletionReason> = new Set<CompletionReason>([
  'crashed',
  'app_quit',
  'interrupted',
  'project_session_interrupted',
]);

export function isResumableReason(reason: string | null | undefined): boolean {
  return !!reason && RESUMABLE_REASONS.has(reason as CompletionReason);
}

export interface Session {
  id: string;
  prompt: string;
  status: SessionStatus;
  workingDir: string;
  provider: string | null;
  agentId: string | null;
  errorMessage: string | null;
  completionReason: CompletionReason | null;
  createdAt: number;
  updatedAt: number;
  /** Pipeline execution state. Present when the session was started via a command pipeline. */
  pipeline?: PipelineExecution | null;
}

// =============================================================================
// Pipelines
// =============================================================================

export type StepStatus = 'pending' | 'running' | 'succeeded' | 'failed' | 'skipped';
export type StepType = 'command' | 'ai_handoff';
export type PipelineKind = 'rebase' | 'squash';

export interface PipelineStepStatus {
  label: string;
  stepType: StepType;
  status: StepStatus;
  output: string | null;
  error: string | null;
  startedAt: number | null;
  completedAt: number | null;
}

export interface PipelineExecution {
  kind?: PipelineKind | null;
  steps: PipelineStepStatus[];
  currentStep: number;
  completedWithoutAi: boolean;
}

/** Payload emitted by the `pipeline-step-changed` Tauri event. */
export interface PipelineStepPayload {
  sessionId: string;
  stepIndex: number;
  label: string;
  stepType: StepType;
  status: StepStatus;
  output: string | null;
  error: string | null;
  startedAt: number | null;
  completedAt: number | null;
}

export type MessageRole = 'user' | 'assistant' | 'tool_call' | 'tool_result';

export interface SessionMessage {
  id: number;
  sessionId: string;
  role: MessageRole;
  content: string;
  createdAt: number;
  /** Image IDs attached to this message (user messages only). */
  imageIds?: string[];
}

// =============================================================================
// Branch sessions
// =============================================================================

export type BranchSessionType = 'note' | 'commit' | 'review';
export type BranchSessionLaunchStatus = 'running' | 'queued';

export interface BranchSessionLaunchContext {
  source: 'diff_viewer';
  scope: 'branch' | 'commit';
  commitSha: string;
  reviewId?: string | null;
}

export interface BranchSessionResponse {
  sessionId: string;
  artifactId: string;
  sessionStatus: BranchSessionLaunchStatus;
}

// =============================================================================
// Session status event payload
// =============================================================================

/** Payload emitted by the `session-status-changed` Tauri event. */
export interface SessionStatusPayload {
  sessionId: string;
  status: SessionStatus;
  errorMessage?: string | null;
  completionReason?: CompletionReason | null;
  branchId?: string;
  projectId?: string;
  sessionType?: string;
  isAutoReview?: boolean;
}

// =============================================================================
// Store status
// =============================================================================

/** Returned by get_store_status when the database needs a reset or is too new. */
export interface StoreIncompatibility {
  /** App version that last used this database (e.g. "0.1.0"). */
  dbAppVersion: string;
  /** Version of this build (e.g. "0.2.0"). */
  appVersion: string;
  /** "needs_reset" = old DB, offer wipe. "too_new" = newer DB, suggest update. */
  kind: 'needs_reset' | 'too_new';
}

// =============================================================================
// Diff, Review, and Annotation types — re-exported from shared package
// =============================================================================

export type {
  Span,
  FileContent,
  File,
  FileDiffSummary,
  Alignment,
  FileDiff,
  DiffFilesResponse,
  CommentAuthor,
  CommentType,
  Comment,
  Review,
  LineSpan,
  AnnotationCategory,
  SmartDiffAnnotation,
  DiffCommands,
  ReviewCommands,
} from '@builderbot/diff-viewer/types';

// =============================================================================
// Hashtag references
// =============================================================================

export interface HashtagItem {
  type: 'note' | 'commit' | 'review' | 'project-note' | 'image';
  id: string;
  title: string;
  color: string;
  bgColor: string;
  subtitle?: string;
  branchName?: string;
  repoSlug?: string;
  repoSubpath?: string | null;
}

// =============================================================================
// Blox workspace types
// =============================================================================

/** Result of polling a remote workspace's status. */
export interface PollWorkspaceResult {
  status: string;
  workstationId: number | null;
}

/** Workspace info returned from `blox ws info`. */
export interface WorkspaceInfo {
  name: string;
  status: string | null;
  [key: string]: unknown;
}
