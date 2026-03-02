/**
 * Shared diff viewer types.
 *
 * These types match the backend serde output from the git-diff crate.
 * All types use camelCase (matching the backend's #[serde(rename_all = "camelCase")]).
 */

// =============================================================================
// Diff types (matching backend git-diff crate serde output)
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

/** Who authored a comment. */
export type CommentAuthor = 'user' | 'agent';

/** The type/severity of a review comment. */
export type CommentType = 'information' | 'suggestion' | 'warning' | 'issue';

/** A comment attached to a specific location in a file. */
export interface Comment {
  id: string;
  path: string;
  span: Span;
  content: string;
  author: CommentAuthor;
  /** The type/severity of this comment. Null for user-authored comments. */
  commentType: CommentType | null;
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

// =============================================================================
// Backend command interfaces (apps provide their own implementations)
// =============================================================================

/** Interface for fetching diff data from the backend. */
export interface DiffCommands {
  getDiffFiles(
    branchId: string,
    commitSha?: string,
    scope?: 'branch' | 'commit'
  ): Promise<DiffFilesResponse>;

  getFileDiff(
    branchId: string,
    commitSha: string,
    scope: 'branch' | 'commit',
    path: string
  ): Promise<FileDiff>;

  getFileAtRef(branchId: string, refName: string, path: string): Promise<File>;
}

/** Interface for review persistence commands. */
export interface ReviewCommands {
  ensureReview(branchId: string, commitSha: string, scope: 'branch' | 'commit'): Promise<Review>;

  findReview(
    branchId: string,
    commitSha: string,
    scope: 'branch' | 'commit'
  ): Promise<Review | null>;

  /** Get a review by its primary key. */
  getReview(reviewId: string): Promise<Review | null>;

  addComment(
    reviewId: string,
    path: string,
    spanStart: number,
    spanEnd: number,
    content: string
  ): Promise<Comment>;

  updateComment(commentId: string, content: string): Promise<void>;
  deleteComment(commentId: string): Promise<void>;

  markReviewed(reviewId: string, path: string): Promise<void>;
  unmarkReviewed(reviewId: string, path: string): Promise<void>;

  addReferenceFile(reviewId: string, path: string): Promise<void>;
  removeReferenceFile(reviewId: string, path: string): Promise<void>;
}
