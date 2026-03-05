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
  createdAt: number;
  updatedAt: number;
}

export interface RecentRepo {
  id: string;
  githubRepo: string;
  subpath: string | null;
  lastUsedAt: number;
}

export interface GitHubRepo {
  name: string;
  nameWithOwner: string;
  description: string | null;
  isPrivate: boolean;
  updatedAt: string;
}

export type BranchType = 'local' | 'remote';
export type WorkspaceStatus = 'starting' | 'running' | 'stopped' | 'error';

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
}

export interface CommitTimelineItem {
  /** DB id — present for pending/failed commits so they can be deleted by id. */
  id: string | null;
  sha: string;
  shortSha: string;
  subject: string;
  author: string;
  /** Unix timestamp in seconds */
  timestamp: number;
  sessionId: string | null;
  sessionStatus: string | null;
}

export interface NoteTimelineItem {
  id: string;
  title: string;
  content: string;
  sessionId: string | null;
  sessionStatus: string | null;
  createdAt: number;
  updatedAt: number;
}

export interface ReviewTimelineItem {
  id: string;
  commitSha: string;
  scope: string;
  sessionId: string | null;
  sessionStatus: string | null;
  title: string | null;
  commentCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface ImageTimelineItem {
  id: string;
  filename: string;
  mimeType: string;
  sizeBytes: number;
  sessionId: string | null;
  sessionStatus: string | null;
  createdAt: number;
}

export interface BranchTimeline {
  commits: CommitTimelineItem[];
  notes: NoteTimelineItem[];
  reviews: ReviewTimelineItem[];
  images: ImageTimelineItem[];
}

export interface Image {
  id: string;
  branchId: string;
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

// =============================================================================
// GitHub types (for PR/Issue picker)
// =============================================================================

export interface PullRequest {
  number: number;
  title: string;
  author: string;
  baseRef: string;
  headRef: string;
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
}

export interface ProjectSessionResponse {
  sessionId: string;
  noteId: string;
}

// =============================================================================
// Sessions
// =============================================================================

export type SessionStatus = 'running' | 'completed' | 'error' | 'cancelled';

export interface Session {
  id: string;
  prompt: string;
  status: SessionStatus;
  agentId: string | null;
  errorMessage: string | null;
  createdAt: number;
  updatedAt: number;
}

export type MessageRole = 'user' | 'assistant' | 'tool_call' | 'tool_result';

export interface SessionMessage {
  id: number;
  sessionId: string;
  role: MessageRole;
  content: string;
  createdAt: number;
}

// =============================================================================
// Branch sessions
// =============================================================================

export type BranchSessionType = 'note' | 'commit' | 'review';

export interface BranchSessionResponse {
  sessionId: string;
  artifactId: string;
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
