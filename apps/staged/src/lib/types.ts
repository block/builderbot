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

// =============================================================================
// Project tags (src-tauri/src/store/tags.rs)
// =============================================================================

/** The fixed tag palette, themed through the `--project-status-*` tokens. */
export type TagColor = 'green' | 'red' | 'amber' | 'blue' | 'purple' | 'cyan' | 'gray';

/**
 * A named set of tags, of which a project has at most one. At most one group
 * is the icon group: its tag stands in for the computed PR status icon.
 */
export interface TagGroup {
  id: string;
  name: string;
  /** Dense 0..n-1 order of groups. */
  position: number;
  isIconGroup: boolean;
  /** The group's board is pinned to the projects sidebar. Any number may be. */
  showInSidebar: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface Tag {
  id: string;
  /** Null for an ungrouped tag, which projects add and remove freely. */
  groupId: string | null;
  name: string;
  color: TagColor;
  /** Kebab-case Lucide icon name, or null for a coloured dot. */
  icon: string | null;
  /** Dense 0..n-1 order within its group, or among ungrouped tags. */
  position: number;
  createdAt: number;
  updatedAt: number;
}

export interface ProjectTag {
  projectId: string;
  tagId: string;
}

/** The whole tag catalog plus every assignment, as `get_tag_state` returns it. */
export interface TagState {
  groups: TagGroup[];
  tags: Tag[];
  assignments: ProjectTag[];
}

/** A tag to create; the backend picks the least-used colour when none is given. */
export interface NewTag {
  name: string;
  color?: TagColor | null;
  icon?: string | null;
}

/** What `deleteTagGroup` does with the group's tags. */
export type DeleteTagGroupMode = 'ungroup' | 'deleteTags';

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
  /** The PR's title on GitHub as of the last fetch; null until the first one. */
  prTitle: string | null;
  /**
   * The push armed to run once the branch and its project are idle ("Push
   * after current work"), or null when none is.
   */
  deferredPush: DeferredPushName | null;
  /** When the deferred push was armed (ms since epoch); null when none is. */
  deferredPushRequestedAt: number | null;
  /** Number of finalized commits on this branch (populated by list_branches_for_project). */
  commitCount?: number;
}

/** The four deferred pushes, as the backend names them (`store::DeferredPush`). */
export type DeferredPushName = 'push' | 'force' | 'push-no-verify' | 'force-no-verify';

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

/** Any tag catalog or assignment write. Small enough to refetch whole. */
export type TagsChangedEvent = Record<string, never>;

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
  /**
   * The session's error message, for a failed row's copy (a deleted
   * workstation reads differently from a crash).
   */
  sessionErrorMessage?: string | null;
  /**
   * Kind of the git pipeline behind this row's session. Only set on pending
   * rows: it is how a queued/running rebase or squash is told apart from a
   * plain commit session, since the subject is a display label an agent-pushed
   * ACP title can replace mid-pipeline.
   */
  pipelineKind?: PipelineKind | null;
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
  /**
   * The session's error message, for a failed row's copy (a deleted
   * workstation reads differently from a crash).
   */
  sessionErrorMessage?: string | null;
  createdAt: number;
  updatedAt: number;
  completedAt: number | null;
  suggestedNextCommitStep: string | null;
  suggestedNextNoteStep: string | null;
  subtype: NoteSubtype;
  /** When the session's chat last got a reply it shows. */
  lastChatMessageAt: number | null;
  /** When the user last read the session's chat. */
  chatReadAt: number | null;
  /** When the user last opened the note; see `isNoteUnread`. */
  readAt: number | null;
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
  /** Set on a child note aggregated under a project note, which branch timelines hide. */
  parentProjectNoteId: string | null;
  subtype: NoteSubtype;
  /** When the user last opened the note; see `isNoteUnread`. */
  readAt: number | null;
}

export interface ReviewTimelineItem {
  id: string;
  commitSha: string;
  scope: 'branch' | 'commit';
  sessionId: string | null;
  sessionStatus: string | null;
  sessionProvider: string | null;
  completionReason: string | null;
  /**
   * The session's error message, for a failed row's copy (a deleted
   * workstation reads differently from a crash).
   */
  sessionErrorMessage?: string | null;
  title: string | null;
  /** Active non-`information` comments (what the detail view lists as comments). */
  commentCount: number;
  /** Active `information` comments (rendered as hold-to-reveal annotations). */
  annotationCount: number;
  /** Subset of `commentCount` typed `warning`. */
  warningCount: number;
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

/** Reply of `get_attachment_thumbnail`; see `attachmentPreview.ts`. */
export interface AttachmentThumbnail {
  /** The image's own data URL, or a PNG of a PDF's first page; null when a PDF could not be rendered. */
  dataUrl: string | null;
  mimeType: string;
  filename: string;
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
  /** Null for videos, PDFs, and raster files whose headers could not be read. */
  dimensions: ImageDimensions | null;
}

/**
 * Pixel size of a stored raster image and the density its own metadata
 * declares (PNG `pHYs`, JPEG JFIF/EXIF resolution), so a 1320x960 Retina
 * capture lays out at 660x480. `density` is 1..=4; 1 when the file says
 * nothing.
 */
export interface ImageDimensions {
  width: number;
  height: number;
  density: number;
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
// Project transfer (src-tauri/src/project_transfer/)
// =============================================================================

/** Rust `TransferDirection`: `toRemote` moves a local project to Blox. */
export type ProjectTransferDirection = 'toRemote' | 'toLocal';

/** Rust `TransferIssue`: a stable `code` for the UI, and a sentence for the user. */
export interface ProjectTransferIssue {
  code: string;
  message: string;
}

/**
 * Rust `RepoTransferState`. `nothingToCarry`: no checkout (no worktree, a
 * stopped workspace), so the branch starts from origin at the destination.
 * `unknown`: not known until the transfer resumes a suspended workspace.
 */
export type ProjectTransferRepoState = 'carry' | 'nothingToCarry' | 'unknown';

/** Rust `RepoPreflight`: one branch, on one of the project's repos. */
export interface ProjectTransferRepoPreflight {
  branchId: string;
  projectRepoId: string | null;
  githubRepo: string;
  subpath: string | null;
  branchName: string;
  /** The name it lands under: differs when the local clone already uses `branchName`. */
  destBranchName: string;
  state: ProjectTransferRepoState;
  head: string | null;
  /** The upstream as git abbreviates it, e.g. `origin/feature`. */
  upstream: string | null;
  /** Commits on no remote-tracking ref. */
  unpushedCommits: number;
  staged: number;
  unstaged: number;
  untracked: number;
  conflicted: number;
  untrackedBytes: number;
  /** Ignored files outside ignored directories (`.env`, `debug.log`), each counted. A
   *  transfer backs them up before removing the source, and carries them when asked. */
  ignoredFiles: number;
  /** Their total size. Over the cap the backend warns `ignored_too_large`: they are
   *  neither carried nor backed up. */
  ignoredBytes: number;
  /** Directories matched by an ignore pattern (`node_modules/`, `.venv/`), each counted
   *  once; they are never carried or backed up. */
  ignoredDirs: number;
  /** Git LFS objects the source has, carried beside the bundle (counting modified and
   *  untracked LFS files, which the snapshot stores as new objects). */
  lfsPresent: number;
  lfsPresentBytes: number;
  /** LFS objects the source never downloaded; the destination fetches them from origin
   *  if it can. */
  lfsMissing: number;
  lfsMissingBytes: number;
  hasUncommittedChanges: boolean;
  /** The checkout is still being read: the counts above are partial. Only
   *  set on the partial results a streamed preflight emits. */
  scanning: boolean;
  blockers: ProjectTransferIssue[];
  warnings: ProjectTransferIssue[];
}

/** Rust `ProjectTransferPreflight`, from `get_project_transfer_preflight`. */
export interface ProjectTransferPreflight {
  projectId: string;
  from: ProjectLocation;
  to: ProjectLocation;
  direction: ProjectTransferDirection;
  /** The source workspace of a transfer to local; null to remote. */
  workspaceName: string | null;
  repos: ProjectTransferRepoPreflight[];
  blockers: ProjectTransferIssue[];
  warnings: ProjectTransferIssue[];
  /** No blockers, project-level or on any repo. */
  canTransfer: boolean;
  /** Some repo has staged, unstaged or untracked changes. */
  hasUncommittedChanges: boolean;
  /** Some repo is still being read (see ProjectTransferRepoPreflight.scanning). */
  scanning: boolean;
}

/**
 * Rust `PreflightScanEvent`, the `project-transfer-preflight` event a
 * `start_project_transfer_preflight` scan emits: a partial preflight each
 * time a checkout's read lands, then one with `done`. A cancelled scan ends
 * with `done` and `cancelled`; a failed one with `done` and `error`.
 */
export interface ProjectTransferPreflightEvent {
  projectId: string;
  scanId: string;
  preflight: ProjectTransferPreflight | null;
  done: boolean;
  cancelled: boolean;
  error: string | null;
}

/** Rust `TransferPhase`, in the order a transfer reaches them. */
export type ProjectTransferPhase =
  | 'checking'
  | 'preparing_destination'
  | 'snapshotting'
  | 'moving'
  | 'applying'
  /** Switching the database over: the last point a cancel is honoured. */
  | 'committing'
  | 'tearing_down'
  | 'completed'
  | 'rolling_back'
  | 'failed';

/** Rust `TransferProgress`, the `project-transfer-progress` event payload. */
export interface ProjectTransferProgress {
  projectId: string;
  transferId: string;
  direction: ProjectTransferDirection;
  phase: ProjectTransferPhase;
  /** The branch the phase is about, when it is about one. */
  branchId: string | null;
  projectRepoId: string | null;
  detail: string | null;
  /** The bundle's size, while `moving`. */
  bytes: number | null;
}

/** Rust `RepoTransferResult`: one branch, after the transfer. */
export interface ProjectTransferRepoResult {
  branchId: string;
  projectRepoId: string | null;
  githubRepo: string;
  /** Its name at the destination. */
  branchName: string;
  previousBranchName: string;
  /** Whether there was a checkout to carry. */
  carried: boolean;
  head: string | null;
  uncommittedRestored: boolean;
  /** Uncommitted changes stayed behind, in the backup only. */
  uncommittedLeftBehind: boolean;
  retriedWithoutExclusions: boolean;
  /** The ref holding the source's snapshot in the local clone (transfer to remote). */
  backupRef: string | null;
  warnings: ProjectTransferIssue[];
}

/** Rust `ProjectTransferResult`, from `transfer_project`. */
export interface ProjectTransferResult {
  projectId: string;
  transferId: string;
  direction: ProjectTransferDirection;
  location: ProjectLocation;
  /** Created by a transfer to remote, deleted by one to local. */
  workspaceName: string | null;
  repos: ProjectTransferRepoResult[];
  /** What tearing the source down could not do (`worktree_left`, `workspace_left`). */
  warnings: ProjectTransferIssue[];
  /** `~/.staged/transfers/<transfer id>`, when anything was carried. */
  backupDir: string | null;
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
  /** When the user last opened the note; see `isNoteUnread`. */
  readAt: number | null;
  sessionStatus: string | null;
  completionReason: string | null;
  /** When the session's chat last got a reply it shows. */
  lastChatMessageAt: number | null;
  /** When the user last read the session's chat. */
  chatReadAt: number | null;
}

export interface ProjectSessionResponse {
  sessionId: string;
  noteId: string;
}

// =============================================================================
// Project search
// =============================================================================

export type ProjectSearchItemKind = 'note' | 'projectNote' | 'review';

/** One titled item of a project in the search index: titles only, no bodies. */
export interface ProjectSearchItem {
  /** For branch notes and reviews, the branch's current project. */
  projectId: string;
  /** The branch a note or review is on; null for project notes. */
  branchId: string | null;
  kind: ProjectSearchItemKind;
  id: string;
  title: string;
  updatedAt: number;
}

// =============================================================================
// Sessions
// =============================================================================

export type SessionStatus =
  'queued' | 'running' | 'disconnected' | 'completed' | 'error' | 'cancelled';

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
  /**
   * The session's remote (Blox) agent process was lost under it — its output
   * ended, or bloxlet reports it stopped. The error message names the cause.
   */
  | 'remote_agent_lost'
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
  'remote_agent_lost',
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
  /**
   * The control-server actor that started this session through stagedctl,
   * when one did: `session:<session-id>` for a session Staged spawned,
   * `app:<grant-id>` for an outside client the user approved. Absent for
   * sessions started from the app's own UI.
   */
  startedBy?: string | null;
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
  /** Whether a `push` pipeline skips the pre-push hook (`--no-verify`). Absent
   *  for the other kinds. */
  pushNoVerify?: boolean;
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

/**
 * The bodies of one tool row, fetched on demand when the transcript shipped
 * them as `$deferred` markers (see `toolCallBodies.ts`). Field names match
 * {@link SessionMessage} so a body slots straight into the row it came from.
 */
export interface ToolCallBody {
  messageId: number;
  acpRawInput?: unknown;
  acpRawOutput?: unknown;
  acpContent?: unknown;
}

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
   *
   * Only the ACP *metadata* rows carry it — the `content: ''` rows from
   * `getSessionAcpMetadataMessages`, written alongside each chunk. The visible
   * transcript row is appended separately and has no ACP metadata of its own,
   * so reading this field off assistant text always yields `undefined`. To
   * badge visible output as a continuation, join the text to its metadata row
   * through `acpMessageId`; for a continuation that id is the synthesized
   * `background-continuation-<n>` when the agent named no message of its own.
   *
   * The value is `background-continuation`, optionally refined with the
   * bridge's own cycle kind and the name of the task that woke it
   * (`background-continuation:task-notification:Run the tests`), so match on
   * the `background-continuation` prefix rather than the whole string.
   *
   * The task name is the agent's — for a background shell it is the command —
   * so it can contain colons of its own and may be a clipped prefix ending in
   * `…`. Neither `split(':')` nor `split(':', 3)` reads it back (the latter
   * drops everything past the third field rather than keeping it); match
   * `/^([^:]+):([^:]+):(.*)$/` or slice at the second `indexOf(':')`. Treat
   * the recovered name as a hint, not a key to match a task by.
   */
  acpOrigin?: string;
}

/** The session pane's poll cursors (`sessionPollRequest` in `messagePoll.ts`). */
export interface SessionPollRequest {
  /** Visible rows with `id >= sinceId` (inclusive: the tail may still grow). */
  sinceId: number;
  /** Visible rows re-read by id: an assistant row extended behind tool rows. */
  refetchIds: number[];
  /** Metadata rows with `id > metadataSinceId`. */
  metadataSinceId: number;
  /** Unsettled tool rows re-read by id. */
  metadataRefetchIds: number[];
}

/** One poll tick, read from a single backend snapshot. */
export interface SessionPoll {
  session: Session | null;
  queuedMessages: QueuedSessionMessage[];
  messages: SessionMessage[];
  acpMetadataMessages: SessionMessage[];
}

/**
 * Whether a message typed for a session can reach its agent now (the chat
 * composer's "Send" mode) instead of waiting in the queue. Computed by the
 * backend from the same gate `continue_repo_session` applies before delivering
 * to a running session; `reason` says why not.
 */
export interface SessionLiveDelivery {
  supported: boolean;
  /**
   * When `supported` is false, whether asking again while the session stays
   * running could get a yes: true while the agent's connection is still coming
   * up, false when another Staged instance owns the session or it is not
   * running. Always false when `supported` is true.
   */
  retryable: boolean;
  reason?: string;
}

/**
 * How `send_session_message_now` delivered a message, or why it did not.
 * Mirrors the Rust `ContinueOutcome::delivery()` spellings, which the
 * `continue_repo_session` MCP tool shares; `resumed` and `not_resumable` are
 * only reached through the tool, since the command never resumes a stopped
 * session (it answers `not_running` instead).
 */
export type SessionMessageDeliveryKind =
  | 'injected'
  | 'interrupted_and_reprompted'
  | 'started_new_turn'
  | 'resumed'
  | 'rejected_queued'
  | 'blocked'
  | 'not_resumable'
  | 'not_running';

export interface SessionMessageDelivery {
  delivery: SessionMessageDeliveryKind;
  /**
   * Set when the message was not delivered (`blocked`, `not_resumable`,
   * `not_running`).
   */
  reason?: string;
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
  /** The queued session that runs after this one, and only if it succeeds:
   *  the force push of "Rebase and force push". Always queued. */
  followUpSessionId?: string;
}

/** What `defer_branch_push` did. */
export interface DeferredPushResponse {
  /** The push is armed and waits for the branch and project to go idle. */
  armed: boolean;
  /** Set when they were already idle: the push that started (or queued) instead. */
  push?: BranchPipelineResponse;
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
  /**
   * Set only on the `cancelled` of a queued session skipped because the
   * session it was gated on did not succeed: it never ran.
   */
  dependencySkipped?: boolean;
}

// =============================================================================
// Reconciling disconnected Blox sessions
// =============================================================================

/**
 * What the backend's reconcile service is doing about a `disconnected` row.
 * Mirrors `ReconcilePhase` in `src-tauri/src/session_runner/reconcile.rs`.
 *
 * - `checking`: asking Blox about the workstation and the session's process,
 *   catching the transcript up, and (when the turn is still running)
 *   following it — the row reads `running` again for that last part.
 * - `stopping`: cancelling the remote turn for a Stop.
 * - `waiting`: the last attempt couldn't settle the row; the next is at
 *   `nextAttemptAt`, or sooner on a "Check now".
 * - `idle`: nothing in progress or pending. The last event for a row that
 *   settled.
 */
export type SessionReconcilePhase = 'checking' | 'stopping' | 'waiting' | 'idle';

/** Why the last reconcile attempt left a row `disconnected`. */
export type SessionReconcileReasonKind =
  | 'unreachable'
  | 'not_authenticated'
  | 'suspended'
  | 'workstation_not_running'
  | 'attach_failed'
  /** Not a failure: the turn is over, but work it started is still running. */
  | 'background_work';

export interface SessionReconcileReason {
  kind: SessionReconcileReasonKind;
  /** Human-readable detail, e.g. the `sq` error. */
  message: string;
}

/**
 * Payload of the `session-reconcile-changed` event, and one entry of
 * `get_session_reconcile_states`: where reconciling one `disconnected`
 * session stands. Emitted on every phase change.
 */
export interface SessionReconcileEvent {
  sessionId: string;
  branchId: string | null;
  projectId: string | null;
  phase: SessionReconcilePhase;
  /** Why the last attempt couldn't settle the row; null before the first failure. */
  reason: SessionReconcileReason | null;
  /** Failed attempts since the row last made progress. */
  attempts: number;
  /** When the backend will next try, in ms since the epoch, while `waiting`. */
  nextAttemptAt: number | null;
}

/**
 * Payload emitted by the `image-created` Tauri event for timeline-visible
 * (non-pending) images, so clients that didn't create the image (e.g. when
 * stagedctl did) can refresh the affected branch timeline.
 */
export interface ImageCreatedPayload {
  imageId: string;
  branchId: string | null;
  projectId: string;
}

/**
 * One entry of the `get_active_sessions` busy-state snapshot: a running,
 * queued or disconnected session projected to its branch/project context.
 * Carries the same discriminators as `SessionStatusPayload` so the snapshot
 * and the `session-status-changed` delta stream describe sessions
 * identically.
 */
export interface ActiveSessionInfo {
  sessionId: string;
  projectId: string | null;
  branchId: string | null;
  sessionType: string | null;
  status: SessionStatus;
  /**
   * Whether the session is in a background hold right now. The hold event is
   * emitted on change only, so this is how a client hydrating mid-hold learns
   * of it. False for a session another instance owns.
   */
  holding: boolean;
}

/**
 * One in-flight repo session spawned by a project session: the branch it
 * works on and the title the branch timeline would give it. Queued children
 * are returned alongside running ones. Backs the badges shown under a running
 * project session's timeline row.
 */
export interface ChildSessionInfo {
  sessionId: string;
  /** One of `'commit'`, `'note'`, `'review'`. */
  sessionType: string;
  /**
   * The only statuses the query returns. `disconnected` is a Blox child that
   * lost its connection and may still be running remotely.
   */
  status: 'queued' | 'running' | 'disconnected';
  /**
   * The child's title as the branch timeline would show it, resolved by the
   * backend with the branch timeline's own rules: a commit's ACP title else
   * its prompt, a note's stored title, a review's stored title else (while
   * running) its ACP title. `null` only for a review with neither yet.
   */
  title: string | null;
  branchId: string;
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

/**
 * Why a PR whose head branch matches a local branch was not linked to it by
 * `detect_branch_pr`. Mirrors `pr_detection::PrRejectionReason`.
 */
export type PrRejectionReason =
  | { kind: 'notOpen'; state: string }
  | { kind: 'wrongBase'; expected: string; actual: string }
  | {
      kind: 'wrongHeadRepo';
      /** Every repository the branch's PR head may live in (parent, plus the fork for a fork setup). */
      expected: string[];
      actual: string | null;
    };

export interface RejectedPrCandidate {
  prNumber: number;
  prUrl: string;
  /** GitHub login of the PR author. */
  author: string;
  draft: boolean;
  reasons: PrRejectionReason[];
  /** Every reason is one the user may override (only `wrongBase` is), so
   *  `link_branch_pr` will accept this PR. */
  linkable: boolean;
}

/**
 * Outcome of the user-triggered "Detect existing PR" lookup. On `found` the
 * backend has already persisted the PR number; clients only render.
 */
export type PrDetectionResult =
  | { kind: 'found'; prNumber: number; prUrl: string }
  | { kind: 'notFound' }
  | { kind: 'rejected'; candidates: RejectedPrCandidate[] };

/** A PR linked by explicit user choice via `link_branch_pr`; already persisted. */
export interface LinkedBranchPr {
  prNumber: number;
  prUrl: string;
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

/**
 * One session the last app quit interrupted, as the launch dialog lists it.
 * Mirrors `launch_resume::InterruptedSession`.
 */
export interface InterruptedSession {
  sessionId: string;
  projectId: string | null;
  branchId: string | null;
  sessionType: string | null;
  /** `commit on fix-login`, as the quit prompt labels sessions. */
  label: string;
  branchName: string | null;
  projectName: string | null;
  /**
   * `resume`: its conversation is restored and it continues (at once, or in
   * its turn on the branch queue if the branch is busy). `restart`: it never
   * reached its agent, so it is re-queued and runs from the beginning.
   * `manual`: listed only; `reason` says why it is not restarted.
   */
  disposition: 'resume' | 'restart' | 'manual';
  reason: string | null;
  /** When the quit ended it (ms since epoch). */
  interruptedAt: number;
}

/**
 * The set offered per launch by `get_interrupted_sessions`, until it is
 * answered. Repo sessions are restarted before project sessions (see
 * `launch_resume`).
 */
export interface LaunchRecovery {
  repoSessions: InterruptedSession[];
  projectSessions: InterruptedSession[];
}

// =============================================================================
// Connected apps (stagedctl grants)
// =============================================================================

/**
 * A `stagedctl connect` request awaiting the user's answer. Mirrors
 * `ControlAuthorizationRequest` in `control_server/enrol.rs`; carried by the
 * `control-authorization-requested` event and `list_pending_control_authorizations`.
 */
export interface ControlAuthorizationRequest {
  requestId: string;
  /** The name the CLI passed as `--client`. */
  client: string;
  /** `null` = every project. */
  projectId: string | null;
  projectName: string | null;
  access: 'read' | 'write';
  /** The requesting process and its parent's command name, shown as information only. */
  peerPid: number | null;
  peerParent: string | null;
  /**
   * The asking process's parent has already exited (or could not be read):
   * it may have been backgrounded from an agent session. A notice, never a decision.
   */
  orphaned: boolean;
  /** ms since the epoch. */
  requestedAt: number;
  /** ms since the epoch; the request lapses unanswered after this. */
  expiresAt: number;
}

/**
 * An approved client as the Settings "Connected apps" panel lists it.
 * Mirrors `ControlGrantSummary` in `control_server/consent.rs`.
 */
export interface ControlGrantSummary {
  id: string;
  client: string;
  /** `null` = every project. */
  projectId: string | null;
  /** The scoped project's current name, when it still exists. */
  projectName: string | null;
  access: 'read' | 'write';
  /** ms since the epoch. */
  createdAt: number;
  /** ms since the epoch; renewed on every successful reconnect. */
  expiresAt: number;
  /** ms since the epoch, or null when never used since approval. */
  lastUsedAt: number | null;
  /** Whether the client holds a live credential in this run of the app. */
  live: boolean;
}

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
