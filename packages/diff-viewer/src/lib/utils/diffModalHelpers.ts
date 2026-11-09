import type { Comment, CommentType, FileDiffSummary } from '../types';
import { fileSummaryPath } from '../state/diffViewerState.svelte';

export interface FileEntry {
  path: string;
  status: 'added' | 'deleted' | 'modified' | 'renamed';
  addedLines?: number | null;
  deletedLines?: number | null;
  isReviewed: boolean;
  commentCount: number;
  /** The distinct comment types present on this file (e.g. 'warning', 'suggestion'). */
  commentTypes: CommentType[];
}

export interface TreeNode {
  name: string;
  path: string;
  isDir: boolean;
  children: TreeNode[];
  file?: FileEntry;
}

/** Trim whitespace and surrounding slashes from a repo subpath; empty means none. */
function normalizeSubpath(subpath: string | null | undefined): string | null {
  const trimmed = (subpath ?? '').trim().replace(/^\/+|\/+$/g, '');
  return trimmed === '' ? null : trimmed;
}

/**
 * Resolve a comment's stored path to the diff file it belongs to.
 *
 * Diff file paths are repo-root-relative, but an agent reviewing from inside
 * a repo subpath may write paths relative to that subpath. Matching is strict
 * so that same-named files in different directories (e.g. `VISION.md` and
 * `docs/VISION.md`) never pick up each other's comments:
 *
 * 1. An exact match always wins.
 * 2. Only when a subpath is configured, try exactly `<subpath>/<path>` and
 *    `<path>` with exactly the `<subpath>/` prefix removed.
 * 3. If no fallback candidate exists, or more than one does, the path is
 *    returned unchanged rather than guessing.
 */
export function resolveCommentFilePath(
  commentPath: string,
  filePaths: ReadonlySet<string>,
  subpath?: string | null
): string {
  if (filePaths.has(commentPath)) return commentPath;

  const prefix = normalizeSubpath(subpath);
  if (!prefix) return commentPath;

  const candidates = new Set<string>();
  const joined = `${prefix}/${commentPath}`;
  if (filePaths.has(joined)) candidates.add(joined);
  if (commentPath.startsWith(`${prefix}/`)) {
    const stripped = commentPath.slice(prefix.length + 1);
    if (filePaths.has(stripped)) candidates.add(stripped);
  }

  if (candidates.size !== 1) return commentPath;
  const [resolved] = candidates;
  return resolved;
}

export function fileStatus(summary: FileDiffSummary): 'added' | 'deleted' | 'modified' | 'renamed' {
  if (!summary.before) return 'added';
  if (!summary.after) return 'deleted';
  if (summary.before !== summary.after) return 'renamed';
  return 'modified';
}

export function fileChangeTotal(
  file: Pick<FileDiffSummary | FileEntry, 'addedLines' | 'deletedLines'>
): number | null {
  const added = file.addedLines;
  const deleted = file.deletedLines;
  if (typeof added !== 'number' && typeof deleted !== 'number') return null;
  return Math.max(0, added ?? 0) + Math.max(0, deleted ?? 0);
}

export function fileChangeScale(
  fileTotal: number | null | undefined,
  maxTotalInDiff: number
): number {
  if (!fileTotal || fileTotal <= 0 || maxTotalInDiff <= 0) return 0;
  return Math.min(1, Math.log1p(fileTotal) / Math.log1p(maxTotalInDiff));
}

/** Aggregate comment count and distinct types for a single path. */
interface CommentAgg {
  count: number;
  types: Set<CommentType>;
}

function groupCommentsByPath(comments: Comment[]): Map<string, CommentAgg> {
  const groups = new Map<string, CommentAgg>();
  for (const comment of comments) {
    let agg = groups.get(comment.path);
    if (!agg) {
      agg = { count: 0, types: new Set() };
      groups.set(comment.path, agg);
    }
    agg.count++;
    if (comment.commentType) agg.types.add(comment.commentType);
  }
  return groups;
}

/**
 * Build sidebar entries for the diff's files. Comments are matched to files by
 * exact path, so callers holding subpath-relative comment paths should resolve
 * them first with `resolveCommentFilePath`.
 */
export function buildFileEntries(
  files: FileDiffSummary[],
  reviewedPaths: string[],
  comments: Comment[]
): FileEntry[] {
  const reviewedSet = new Set(reviewedPaths);
  const commentGroups = groupCommentsByPath(comments);

  return files.map((summary) => {
    const path = fileSummaryPath(summary);
    const agg = commentGroups.get(path);

    return {
      path,
      status: fileStatus(summary),
      addedLines: summary.addedLines ?? null,
      deletedLines: summary.deletedLines ?? null,
      isReviewed: reviewedSet.has(path),
      commentCount: agg?.count ?? 0,
      commentTypes: agg ? [...agg.types] : [],
    };
  });
}

export function buildTree(entries: FileEntry[]): TreeNode[] {
  const root: TreeNode[] = [];

  for (const file of entries) {
    const parts = file.path.split('/');
    let currentLevel = root;

    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      const isLast = i === parts.length - 1;
      const pathSoFar = parts.slice(0, i + 1).join('/');

      let existing = currentLevel.find((n) => n.name === part);

      if (!existing) {
        existing = {
          name: part,
          path: pathSoFar,
          isDir: !isLast,
          children: [],
          file: isLast ? file : undefined,
        };
        currentLevel.push(existing);
      }

      if (!isLast) {
        currentLevel = existing.children;
      }
    }
  }

  function sortTree(nodes: TreeNode[]): TreeNode[] {
    nodes.sort((a, b) => {
      if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
      return a.name.localeCompare(b.name);
    });
    for (const node of nodes) {
      if (node.children.length > 0) sortTree(node.children);
    }
    return nodes;
  }

  return sortTree(root);
}

export function compactTree(nodes: TreeNode[]): TreeNode[] {
  return nodes.map((node) => {
    if (node.isDir && node.children.length === 1 && node.children[0].isDir) {
      const child = compactTree(node.children)[0];
      return { ...child, name: node.name + '/' + child.name, path: child.path };
    }
    return { ...node, children: node.isDir ? compactTree(node.children) : [] };
  });
}

export function formatLineRange(span: { start: number; end: number }): string {
  if (span.end === span.start + 1) return `L${span.start + 1}`;
  return `L${span.start + 1}-${span.end}`;
}

export function truncateText(text: string, maxLength = 40): string {
  const singleLine = text.replace(/\n/g, ' ').trim();
  if (singleLine.length <= maxLength) return singleLine;
  return singleLine.slice(0, maxLength).trim() + '...';
}
