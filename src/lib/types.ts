// =============================================================================
// Diff types
// =============================================================================

/** Content of a file - either text lines or binary marker */
export type FileContent = { type: 'text'; lines: string[] } | { type: 'binary' };

/** A file with its path and content */
export interface File {
  path: string;
  content: FileContent;
}

/** A contiguous range of lines (0-indexed, exclusive end) */
export interface Span {
  start: number;
  end: number;
}

/** Maps a region in the before file to a region in the after file */
export interface Alignment {
  before: Span;
  after: Span;
  /** True if this region contains changes */
  changed: boolean;
}

/** The diff for a single file between two states */
export interface FileDiff {
  /** File before the change (null if added) */
  before: File | null;
  /** File after the change (null if deleted) */
  after: File | null;
  /** Alignments mapping regions between before/after */
  alignments: Alignment[];
}

// =============================================================================
// Git types
// =============================================================================

/** Basic repository info */
export interface RepoInfo {
  repo_path: string;
  branch: string | null;
}

/** A git reference for autocomplete */
export interface GitRef {
  name: string;
  ref_type: 'branch' | 'tag' | 'special';
}

// =============================================================================
// GitHub types
// =============================================================================

/** A GitHub pull request */
export interface PullRequest {
  number: number;
  title: string;
  author: string;
  base_ref: string;
  head_ref: string;
  head_sha: string;
  draft: boolean;
  additions: number;
  deletions: number;
  updated_at: string;
}

/** GitHub authentication status */
export interface GitHubAuthStatus {
  authenticated: boolean;
  setup_hint: string | null;
}

/** Result of fetching a PR branch */
export interface PRFetchResult {
  /** The merge-base SHA (use as diff base to show only PR changes) */
  merge_base: string;
  /** The PR head commit SHA */
  head_sha: string;
}

// =============================================================================
// Review types
// =============================================================================

/** Identifies a diff by its two endpoints */
export interface DiffId {
  before: string;
  after: string;
}

/** A diff specification with display label (for UI) */
export interface DiffSpec {
  base: string;
  head: string;
  label: string;
  /** If true, use merge-base of base and head instead of base directly */
  useMergeBase?: boolean;
}

/** A comment attached to a specific location in a file */
export interface Comment {
  id: string;
  path: string;
  /** The line range this comment applies to (0-indexed, exclusive end) */
  span: Span;
  content: string;
}

/** An edit made during review, stored as a unified diff */
export interface Edit {
  id: string;
  path: string;
  diff: string;
}

/** A review attached to a specific diff */
export interface Review {
  id: DiffId;
  reviewed: string[];
  comments: Comment[];
  edits: Edit[];
}

/** Input for creating a new comment */
export interface NewComment {
  path: string;
  span: Span;
  content: string;
}

/** Input for recording a new edit */
export interface NewEdit {
  path: string;
  diff: string;
}
