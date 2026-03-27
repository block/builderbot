import type { Comment } from '../types';

// E-PENPAL-COMMENT-ORDER: tree-order sorting with parent grouping and time-based sibling sort.
export function orderComments(comments: Comment[]): Comment[] {
  if (!comments || comments.length <= 1) return comments;

  const byId: Record<string, boolean> = {};
  comments.forEach((c) => {
    byId[c.id] = true;
  });

  const children: Record<string, Comment[]> = {};
  const roots: Comment[] = [];
  comments.forEach((c) => {
    const parent = c.inReplyTo || '';
    if (parent && byId[parent]) {
      (children[parent] = children[parent] || []).push(c);
    } else {
      roots.push(c);
    }
  });

  // E-PENPAL-COMMENT-ORDER: use workingStartedAt as effective time when present.
  const effectiveTime = (c: Comment) => c.workingStartedAt || c.createdAt;
  const byTime = (a: Comment, b: Comment) => (effectiveTime(a) < effectiveTime(b) ? -1 : 1);
  roots.sort(byTime);
  Object.keys(children).forEach((k) => children[k].sort(byTime));

  const result: Comment[] = [];
  function walk(id: string) {
    (children[id] || []).forEach((c) => {
      result.push(c);
      walk(c.id);
    });
  }
  roots.forEach((c) => {
    result.push(c);
    walk(c.id);
  });
  return result;
}

export function formatTime(isoStr: string): string {
  if (!isoStr) return '';
  const d = new Date(isoStr);
  const now = new Date();
  const diffMs = now.getTime() - d.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return diffMins + 'm ago';
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return diffHours + 'h ago';
  const diffDays = Math.floor(diffHours / 24);
  if (diffDays < 7) return diffDays + 'd ago';
  return d.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}

export function truncateText(text: string, maxLen: number): string {
  if (!text) return '';
  return text.length > maxLen ? text.substring(0, maxLen) + '...' : text;
}
