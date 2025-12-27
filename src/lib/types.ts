export interface FileStatus {
  path: string;
  status: 'modified' | 'added' | 'deleted' | 'renamed' | 'typechange' | 'untracked' | 'unknown';
}

export interface GitStatus {
  staged: FileStatus[];
  unstaged: FileStatus[];
  untracked: FileStatus[];
  branch: string | null;
  repo_path: string;
}

export interface DiffLine {
  line_type: 'context' | 'added' | 'removed';
  lineno: number;
  content: string;
}

export interface DiffHunk {
  old_start: number;
  old_lines: number;
  new_start: number;
  new_lines: number;
  header: string;
  lines: DiffLine[];
}

/** Half-open interval [start, end) of row indices */
export interface Span {
  start: number;
  end: number;
}

/** Source file line numbers for a changed region (1-indexed, inclusive) */
export interface SourceLines {
  /** Lines removed from the "before" file. None if pure addition */
  old_start: number | null;
  old_end: number | null;
  /** Lines added in the "after" file. None if pure deletion */
  new_start: number | null;
  new_end: number | null;
}

/** Maps corresponding regions between before/after panes */
export interface Range {
  before: Span;
  after: Span;
  /** true = region contains changes, false = identical lines */
  changed: boolean;
  /** Source file line numbers (only present for changed ranges) */
  source_lines?: SourceLines;
}

/** Content for one side of the diff */
export interface DiffSide {
  path: string | null;
  lines: DiffLine[];
}

export interface FileDiff {
  status: string;
  is_binary: boolean;
  hunks: DiffHunk[];
  before: DiffSide;
  after: DiffSide;
  ranges: Range[];
}

export interface CommitResult {
  oid: string;
  message: string;
}
