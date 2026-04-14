import type { Comment, CommentType, FileDiffSummary } from '../types';
import { fileSummaryPath } from '../state/diffViewerState.svelte';

export interface FileEntry {
  path: string;
  status: 'added' | 'deleted' | 'modified' | 'renamed';
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

/** Check whether two file paths refer to the same file.
 *  Handles cases where one path is a suffix of the other (e.g. different root prefixes). */
export function pathsMatch(a: string, b: string): boolean {
  if (a === b) return true;
  if (a.endsWith(b)) return a[a.length - b.length - 1] === '/';
  if (b.endsWith(a)) return b[b.length - a.length - 1] === '/';
  return false;
}

export function fileStatus(summary: FileDiffSummary): 'added' | 'deleted' | 'modified' | 'renamed' {
  if (!summary.before) return 'added';
  if (!summary.after) return 'deleted';
  if (summary.before !== summary.after) return 'renamed';
  return 'modified';
}

/** Aggregate comment count and distinct types for a single path. */
interface CommentAgg {
  count: number;
  types: Set<CommentType>;
}

/** Extract the basename (last segment after '/') from a path. */
function basename(path: string): string {
  const idx = path.lastIndexOf('/');
  return idx === -1 ? path : path.slice(idx + 1);
}

/**
 * Pre-groups comments by their basename, then aggregates per unique comment
 * path. This lets us avoid the O(files × comments) nested loop — each file
 * only checks the (usually tiny) set of comment paths that share its basename.
 */
function groupCommentsByBasename(
  comments: Comment[]
): Map<string, Map<string, CommentAgg>> {
  // basename → (commentPath → CommentAgg)
  const groups = new Map<string, Map<string, CommentAgg>>();
  for (const comment of comments) {
    const base = basename(comment.path);
    let byPath = groups.get(base);
    if (!byPath) {
      byPath = new Map();
      groups.set(base, byPath);
    }
    let agg = byPath.get(comment.path);
    if (!agg) {
      agg = { count: 0, types: new Set() };
      byPath.set(comment.path, agg);
    }
    agg.count++;
    if (comment.commentType) agg.types.add(comment.commentType);
  }
  return groups;
}

export function buildFileEntries(
  files: FileDiffSummary[],
  reviewedPaths: string[],
  comments: Comment[]
): FileEntry[] {
  const reviewedSet = new Set(reviewedPaths);
  const filePaths = files.map(fileSummaryPath);
  const commentGroups = groupCommentsByBasename(comments);

  return files.map((summary, i) => {
    const path = filePaths[i];
    let commentCount = 0;
    const commentTypes = new Set<CommentType>();

    // Only inspect comment paths that share this file's basename.
    const candidates = commentGroups.get(basename(path));
    if (candidates) {
      for (const [commentPath, agg] of candidates) {
        if (pathsMatch(commentPath, path)) {
          commentCount += agg.count;
          for (const t of agg.types) commentTypes.add(t);
        }
      }
    }

    return {
      path,
      status: fileStatus(summary),
      isReviewed: reviewedSet.has(path),
      commentCount,
      commentTypes: [...commentTypes],
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
