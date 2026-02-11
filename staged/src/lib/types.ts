/**
 * Frontend types matching backend serde output.
 *
 * All types use camelCase (matching the backend's #[serde(rename_all = "camelCase")]).
 */

export interface Project {
  id: string;
  repoPath: string;
  subpath: string | null;
  createdAt: number;
  updatedAt: number;
}

export type BranchType = 'local' | 'remote';
export type WorkspaceStatus = 'starting' | 'running' | 'stopped' | 'error';

export interface Branch {
  id: string;
  projectId: string;
  branchName: string;
  baseBranch: string;
  prNumber: number | null;
  branchType: BranchType;
  workspaceName: string | null;
  workspaceStatus: WorkspaceStatus | null;
  agent: string | null;
  worktreePath: string | null;
  createdAt: number;
  updatedAt: number;
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
  commentCount: number;
  createdAt: number;
  updatedAt: number;
}

export interface BranchTimeline {
  commits: CommitTimelineItem[];
  notes: NoteTimelineItem[];
  reviews: ReviewTimelineItem[];
}

export interface BranchRef {
  name: string;
  isRemote: boolean;
  remote: string | null;
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

export type BranchSessionType = 'note' | 'commit';

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
// Diff types (matching backend git::types serde output)
// =============================================================================

/** A contiguous range of lines (0-indexed, exclusive end). */
export interface Span {
  start: number;
  end: number;
}

/** Content of a file — either text lines or binary marker. */
export type FileContent = { type: 'Text'; lines: string[] } | { type: 'Binary' };

/** A file with its path and content. */
export interface File {
  path: string;
  content: FileContent;
}

/** Summary of a file in the diff (for file sidebar). */
export interface FileDiffSummary {
  before: string | null;
  after: string | null;
}

/** Maps a region in the before file to a region in the after file. */
export interface Alignment {
  before: Span;
  after: Span;
  /** True if this region contains changes. */
  changed: boolean;
}

/** Full diff content for rendering a single file. */
export interface FileDiff {
  /** File before the change (null if added). */
  before: File | null;
  /** File after the change (null if deleted). */
  after: File | null;
  /** Alignments mapping regions between before/after. */
  alignments: Alignment[];
}

/** Response from get_diff_files including the resolved commit SHA. */
export interface DiffFilesResponse {
  /** Resolved commit SHA (tip for branch scope, or the passed-in SHA). */
  commitSha: string;
  /** Changed files in the diff. */
  files: FileDiffSummary[];
}

// =============================================================================
// Review types
// =============================================================================

/** A comment attached to a specific location in a file. */
export interface Comment {
  id: string;
  path: string;
  span: Span;
  content: string;
  createdAt: number;
}

/** A review anchored to a branch + commit + scope. */
export interface Review {
  id: string;
  branchId: string;
  commitSha: string;
  scope: 'commit' | 'branch';
  sessionId: string | null;
  /** Paths that have been marked as reviewed. */
  reviewed: string[];
  /** Comments attached to specific locations. */
  comments: Comment[];
  /** Paths of reference files. */
  referenceFiles: string[];
  createdAt: number;
  updatedAt: number;
}

// =============================================================================
// Annotation types (render infrastructure — not wired to any AI backend yet)
// =============================================================================

/** A span of lines for AI annotations (0-indexed, exclusive end). */
export interface LineSpan {
  start: number;
  end: number;
}

/** Category of AI annotation. */
export type AnnotationCategory = 'explanation' | 'warning' | 'suggestion' | 'context';

/** A single AI annotation on a diff region. */
export interface SmartDiffAnnotation {
  id: string;
  /** Description of the old state (for before-pane overlays). */
  before_description?: string;
  /** File path this annotation belongs to. */
  file_path?: string;
  /** Span in the 'before' content (undefined if only applies to 'after'). */
  before_span?: LineSpan;
  /** Span in the 'after' content (undefined if only applies to 'before'). */
  after_span?: LineSpan;
  /** The AI commentary. */
  content: string;
  /** Category for styling. */
  category: AnnotationCategory;
}
