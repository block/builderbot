// =============================================================================
// Git Ref types
// =============================================================================

/** A reference to a point in git history (or working tree) */
export type GitRef = { type: 'WorkingTree' } | { type: 'Rev'; value: string };

/** Inferred type of a ref string for display purposes */
export type RefType = 'branch' | 'tag' | 'remote' | 'special';

/**
 * Infer the type of a ref from its string representation.
 * Best-effort heuristic for display icons.
 */
export function inferRefType(ref: string): RefType {
  // Special refs
  if (ref === '@' || ref === 'HEAD' || ref.startsWith('HEAD~') || ref.startsWith('HEAD^')) {
    return 'special';
  }
  // Tags (from refs/tags/ or common tag patterns like v1.0.0)
  if (ref.startsWith('refs/tags/') || /^v?\d+\.\d+/.test(ref)) {
    return 'tag';
  }
  // Remotes (origin/*, upstream/*, etc.)
  if (ref.includes('/') && !ref.startsWith('refs/')) {
    return 'remote';
  }
  // Default to branch
  return 'branch';
}

/** What we're diffing - always base..head */
export interface DiffSpec {
  base: GitRef;
  head: GitRef;
}

/** Convenience constructors for DiffSpec */
export const DiffSpec = {
  /** Uncommitted changes: HEAD..@ */
  uncommitted(): DiffSpec {
    return {
      base: { type: 'Rev', value: 'HEAD' },
      head: { type: 'WorkingTree' },
    };
  },

  /** Last commit: HEAD~1..HEAD */
  lastCommit(): DiffSpec {
    return {
      base: { type: 'Rev', value: 'HEAD~1' },
      head: { type: 'Rev', value: 'HEAD' },
    };
  },

  /** Custom range */
  custom(base: GitRef, head: GitRef): DiffSpec {
    return { base, head };
  },

  /** From two rev strings */
  fromRevs(base: string, head: string): DiffSpec {
    return {
      base: { type: 'Rev', value: base },
      head: { type: 'Rev', value: head },
    };
  },

  /** Display as "base..head" */
  display(spec: DiffSpec): string {
    const baseStr = spec.base.type === 'WorkingTree' ? '@' : spec.base.value;
    const headStr = spec.head.type === 'WorkingTree' ? '@' : spec.head.value;
    return `${baseStr}..${headStr}`;
  },
};

// =============================================================================
// File types
// =============================================================================

/** A contiguous range of lines (0-indexed, exclusive end) */
export interface Span {
  start: number;
  end: number;
}

/** Content of a file - either text lines or binary marker */
export type FileContent = { type: 'Text'; lines: string[] } | { type: 'Binary' };

/** A file with its path and content */
export interface File {
  path: string;
  content: FileContent;
}

/** Summary of a file in the diff (for sidebar) */
export interface FileDiffSummary {
  before: string | null;
  after: string | null;
}

/** Maps a region in the before file to a region in the after file */
export interface Alignment {
  before: Span;
  after: Span;
  /** True if this region contains changes */
  changed: boolean;
}

/** Full diff content for rendering a single file */
export interface FileDiff {
  /** File before the change (null if added) */
  before: File | null;
  /** File after the change (null if deleted) */
  after: File | null;
  /** Alignments mapping regions between before/after */
  alignments: Alignment[];
}

// =============================================================================
// GitHub types
// =============================================================================

/** A pull request from GitHub (for display in picker) */
export interface PullRequest {
  number: number;
  title: string;
  author: string;
  /** Target branch (e.g., "main") */
  base_ref: string;
  /** Source branch (e.g., "feature-x") */
  head_ref: string;
  draft: boolean;
  updated_at: string;
}

/** GitHub authentication status */
export interface GitHubAuthStatus {
  authenticated: boolean;
  /** Help text if not authenticated */
  setup_hint: string | null;
}

/** Result of syncing a review to GitHub */
export interface GitHubSyncResult {
  /** URL to the pending review on GitHub */
  review_url: string;
  /** Number of comments synced */
  comment_count: number;
}

// =============================================================================
// Review types
// =============================================================================

/** Identifies a diff by its two endpoints */
export interface DiffId {
  before: string;
  after: string;
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
  reference_files: string[];
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
