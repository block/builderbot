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

// Store change feed events (src-tauri/src/store_events.rs). Every mutating
// store method publishes one, so these fire for a write made in any window or
// in the backend itself. A null id means the backend couldn't resolve it —
// treat as "refetch the whole surface". An event whose ids are *all* null is
// the feed's lag recovery: it dropped changes it can no longer describe, and
// every one of these fires at once.

export interface ProjectChangedEvent {
  projectId: string | null;
}

export interface BranchChangedEvent {
  branchId: string | null;
  projectId: string | null;
}

export interface NotesChangedEvent {
  branchId: string | null;
  projectId: string | null;
}

export interface ReviewChangedEvent {
  reviewId: string | null;
  branchId: string | null;
}

export interface ReposChangedEvent {
  githubRepo: string | null;
}

export interface CommitTimelineItem {
  /** DB id — present for pending/failed commits so they can be deleted by id. */
  id: string | null;
  sha: string;
  shortSha: string;
  subject: string;
  author: string;
  authorEmail: string;
  /** Unix timestamp in seconds — author time for branch commits, so it survives a rebase. */
  timestamp: number;
  /**
   * Unix timestamp in seconds to sort on, clamped so it can't decrease in
   * branch order. Order only — render `timestamp`.
   */
  sortTimestamp: number;
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
  subtype: NoteSubtype;
}

/**
 * How a note's content came to be. `null` means an agent session produced it;
 * `'written'` means the user authored it directly, so it opens in the editor
 * rather than the read-only viewer.
 */
export type NoteSubtype = 'written' | null;

/** Subtype marking a user-authored note. */
export const WRITTEN_NOTE_SUBTYPE = 'written';

/** A full branch note record, as returned by `get_branch_note_by_session`. */
export interface BranchNote {
  id: string;
  branchId: string;
  sessionId: string | null;
  title: string;
  content: string;
  createdAt: number;
  updatedAt: number;
  completedAt: number | null;
  suggestedNextCommitStep: string | null;
  suggestedNextNoteStep: string | null;
  subtype: NoteSubtype;
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
  /**
   * The turn finished but the session was held open for background work, and
   * the hold's hard cap expired before that work could be confirmed drained —
   * the wait was truncated, so the session is worth nudging.
   */
  | 'held_until_cap'
  /**
   * The turn finished but the wait for its background work was stopped before
   * it could be confirmed drained — the user pressed Stop mid-hold, or the
   * agent process exited under it. Same truncated-wait semantics as
   * `held_until_cap`.
   */
  | 'hold_stopped'
  | 'interrupted'
  | 'project_session_interrupted'
  | 'crashed'
  | 'app_quit'
  | 'unknown';

/**
 * Completion reasons that indicate a session can be resumed.
 *
 * Mirrors `CompletionReason::is_resumable` in `src-tauri/src/store/models.rs`.
 * `turn_complete` is absent on purpose: the agent said it was done.
 */
export const RESUMABLE_REASONS: ReadonlySet<CompletionReason> = new Set<CompletionReason>([
  'crashed',
  'app_quit',
  'interrupted',
  'project_session_interrupted',
  'held_until_cap',
  'hold_stopped',
]);

export function isResumableReason(reason: string | null | undefined): boolean {
  return !!reason && RESUMABLE_REASONS.has(reason as CompletionReason);
}

/**
 * Completion reasons whose *turn* finished — the agent did the work, whatever
 * happened to the wait that followed it.
 *
 * Mirrors `terminal_state_completed_successfully` in
 * `src-tauri/src/session_runner.rs`, which is what gates the backend's own
 * post-completion hooks. `held_until_cap` and `hold_stopped` belong here even
 * though they are also resumable: the two sets deliberately overlap, because a
 * truncated background wait leaves a turn both complete (its output is real)
 * and worth nudging (its background work went unconfirmed). Use this — not an
 * equality check against `turn_complete` — for anything gated on the agent
 * having produced output.
 */
export const COMPLETED_TURN_REASONS: ReadonlySet<CompletionReason> = new Set<CompletionReason>([
  'turn_complete',
  'held_until_cap',
  'hold_stopped',
]);

export function isCompletedTurnReason(reason: string | null | undefined): boolean {
  return !!reason && COMPLETED_TURN_REASONS.has(reason as CompletionReason);
}

export interface AcpConfigValueSelection {
  configId: string;
  valueId: string;
  label?: string | null;
}

export interface AcpConfigSelection {
  model?: AcpConfigValueSelection | null;
  effort?: AcpConfigValueSelection | null;
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
  /** Selected ACP config values to apply before prompting the agent. */
  acpConfigSelection?: AcpConfigSelection | null;
  /** Latest session title pushed by the agent via ACP `session_info_update`. */
  acpTitle: string | null;
  /**
   * Branch this session runs for. Present on pipeline-launched (pr/push)
   * sessions, which link no artifact row; artifact-linked sessions resolve
   * their branch through the artifact instead.
   */
  branchId?: string | null;
}

export type QueuedSessionMessageStatus = 'queued' | 'sending' | 'sent';

export interface QueuedSessionMessage {
  id: string;
  sessionId: string;
  branchId: string | null;
  content: string;
  imageIds: string[];
  status: QueuedSessionMessageStatus;
  lastError: string | null;
  createdAt: number;
  updatedAt: number;
  claimedAt: number | null;
  ownerPid: number | null;
  sentMessageId: number | null;
}

// =============================================================================
// Pipelines
// =============================================================================

export type StepStatus = 'pending' | 'running' | 'succeeded' | 'failed' | 'skipped';
export type StepType = 'command' | 'ai_handoff';
export type PipelineKind = 'rebase' | 'squash' | 'push' | 'pull';

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
  /** Whether a `push` pipeline force-pushes. Absent for the other kinds. */
  pushForce?: boolean;
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
  acpEventKind?: string;
  acpProtocolVersion?: string;
  acpAgentCapabilities?: unknown;
  acpAuthMethods?: unknown;
  acpAgentInfo?: unknown;
  acpMessageId?: string;
  acpToolCallId?: string;
  acpToolKind?: string;
  acpToolStatus?: string;
  acpRawInput?: unknown;
  acpRawOutput?: unknown;
  acpContent?: unknown;
  acpLocations?: unknown;
  acpUsage?: unknown;
  acpSessionInfo?: unknown;
  acpConfigOptions?: unknown;
  acpSessionModeState?: unknown;
  /**
   * Attribution for rows the agent produced outside a live user turn — a
   * background continuation while the session was held open. Absent means the
   * row belongs to a turn the user prompted.
   */
  acpOrigin?: string;
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

/**
 * Result of a branch git pipeline command (rebase, squash, push, force push).
 *
 * These can be requested while the branch already has sessions in flight, in
 * which case the backend queues them and reports `'queued'`.
 *
 * Pull is the odd one out: an idle branch fast-forwards without going through the
 * pipeline runner, so `pullOrQueueBranch` returns `string | null` — the queued
 * session id, or `null` for a pull that already happened.
 */
export interface BranchPipelineResponse {
  sessionId: string;
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
}

/**
 * One entry of the `get_active_sessions` busy-state snapshot: a running or
 * queued session projected to its branch/project context. Carries the same
 * discriminators as `SessionStatusPayload` so the snapshot and the
 * `session-status-changed` delta stream describe sessions identically.
 */
export interface ActiveSessionInfo {
  sessionId: string;
  projectId: string | null;
  branchId: string | null;
  sessionType: string | null;
  status: SessionStatus;
}

/**
 * Payload emitted by the `pr-created` domain event when a completed PR
 * session produced a pull request. The backend has already persisted the PR
 * number and kicked off a status refresh; clients only render.
 */
export interface PrCreatedPayload {
  branchId: string;
  sessionId: string;
  prUrl: string;
  prNumber: number;
}

export type PushCompletedOutcome = 'succeeded' | 'rejectedNonFastForward';

/**
 * Payload emitted by the `push-completed` domain event when a push session
 * completes. On success the backend has already cleared the stale PR status
 * (and emitted `pr-status-cleared`); clients only render.
 */
export interface PushCompletedPayload {
  branchId: string;
  sessionId: string;
  outcome: PushCompletedOutcome;
}

/**
 * The background hold a session is reporting right now, as
 * `get_session_background_hold` answers it.
 *
 * A presentational sub-state of `running`, not a status: the session stays
 * `running` while its agent is held open past turn end for background work.
 *
 * The event below is emitted on *change*, so a pane that mounts mid-hold has
 * already missed every report for it and asks for this instead — otherwise it
 * would render a plain running indicator for the rest of the wait.
 */
export interface SessionBackgroundHoldSnapshot {
  /** False withdraws the wait — a new turn took over, or teardown started. */
  holding: boolean;
  /** Live background tasks the agent is reporting; 0 when not holding. */
  liveTasks: number;
  /**
   * The live tasks by name, when the agent names them (typed asyncTasks
   * announce each spawn's metadata). Older bridges only report the count, so
   * this stays empty and the wait renders as the bare-count row.
   */
  tasks: SessionBackgroundHoldTask[];
}

/**
 * Payload emitted by the `session-background-hold` Tauri event: a
 * `SessionBackgroundHoldSnapshot` plus the routing context clients filter on.
 */
export interface SessionBackgroundHoldPayload extends SessionBackgroundHoldSnapshot {
  sessionId: string;
  branchId?: string | null;
  projectId?: string | null;
}

/**
 * One named background task in a `SessionBackgroundHoldPayload`. The id keys
 * the per-task stop (`stop_session_async_task`); the rest is presentation.
 */
export interface SessionBackgroundHoldTask {
  id: string;
  name?: string | null;
  description?: string | null;
  outputFilePath?: string | null;
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
  CommentSaveStatus,
  CommentActionContext,
  Review,
  LineSpan,
  AnnotationCategory,
  SmartDiffAnnotation,
  DiffCommands,
  ReviewCommands,
} from '@builderbot/diff-viewer/types';

export type GithubButtonState = 'idle' | 'sending' | 'sent' | 'stale';

/**
 * State of a review comment's "Note"/"Commit" action, derived from the linked
 * session's status. `queued` mirrors the timeline's queued state (Clock icon),
 * and `error`/`cancelled` collapse back to `idle` so the user can retry.
 */
export type CommentSessionState = 'idle' | 'queued' | 'running' | 'completed';

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
  /** Optional context used when resolving a rendered hashtag badge click. */
  branchId?: string;
  projectId?: string | null;
  noteContent?: string;
  noteSessionId?: string | null;
  noteUpdatedAt?: number | null;
  imageFilename?: string;
  reviewCommitSha?: string;
  reviewScope?: 'branch' | 'commit';
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
