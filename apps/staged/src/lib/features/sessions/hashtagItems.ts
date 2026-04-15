import type { BranchTimeline, HashtagItem, ProjectNote, Branch, ProjectRepo } from '../../types';
import { getBranchTimeline, listProjectNotes } from '../../commands';

/** Regex matching `#type:id` hashtag tokens in plain text. Use with `new RegExp(source, 'g')` for stateful iteration. */
export const HASHTAG_TOKEN_RE = /#(note|commit|review|project-note):([^\s]+)/g;

/** Human-readable labels for each hashtag type. */
export const hashtagTypeLabels: Record<string, string> = {
  note: 'Note',
  commit: 'Commit',
  review: 'Review',
  'project-note': 'Note',
};

/** CSS custom-property names for each hashtag type's foreground and background colors. */
export const hashtagTypeColors: Record<string, { color: string; bg: string }> = {
  note: { color: '--note-color', bg: '--note-bg' },
  commit: { color: '--commit-color', bg: '--commit-bg' },
  review: { color: '--review-color', bg: '--review-bg' },
  'project-note': { color: '--note-color', bg: '--note-bg' },
};

/**
 * Build hashtag items for a single branch scope (+ optional project notes).
 */
export async function buildBranchHashtagItems(
  branchId: string,
  projectId: string | null
): Promise<HashtagItem[]> {
  const items: HashtagItem[] = [];

  const [timeline, projectNotes] = await Promise.all([
    getBranchTimeline(branchId),
    projectId ? listProjectNotes(projectId) : Promise.resolve([]),
  ]);

  items.push(...timelineToHashtagItems(timeline));
  items.push(...projectNotesToHashtagItems(projectNotes));

  return items;
}

/**
 * Build hashtag items for a project scope (all branches + project notes).
 */
export async function buildProjectHashtagItems(
  projectId: string,
  branches: Branch[],
  reposById?: Map<string, ProjectRepo>
): Promise<HashtagItem[]> {
  const items: HashtagItem[] = [];

  const [timelines, projectNotes] = await Promise.all([
    Promise.all(
      branches.map((b) => getBranchTimeline(b.id).then((t) => ({ branch: b, timeline: t })))
    ),
    listProjectNotes(projectId),
  ]);

  for (const { branch, timeline } of timelines) {
    const repo = branch.projectRepoId && reposById ? reposById.get(branch.projectRepoId) : null;
    const repoSlug = repo?.githubRepo;
    items.push(...timelineToHashtagItems(timeline, branch.branchName, repoSlug));
  }

  items.push(...projectNotesToHashtagItems(projectNotes));

  return items;
}

export function timelineToHashtagItems(
  timeline: BranchTimeline,
  branchName?: string,
  repoSlug?: string
): HashtagItem[] {
  const items: HashtagItem[] = [];

  for (const note of timeline.notes) {
    if (!note.title.trim()) continue;
    items.push({
      type: 'note',
      id: note.id,
      title: note.title,
      color: '--note-color',
      bgColor: '--note-bg',
      branchName,
      repoSlug,
    });
  }

  for (const commit of timeline.commits) {
    items.push({
      type: 'commit',
      id: commit.sha,
      title: `${commit.shortSha} ${commit.subject}`,
      color: '--commit-color',
      bgColor: '--commit-bg',
      branchName,
      repoSlug,
    });
  }

  for (const review of timeline.reviews) {
    const title = review.title || review.commitSha.slice(0, 7);
    items.push({
      type: 'review',
      id: review.id,
      title,
      color: '--review-color',
      bgColor: '--review-bg',
      branchName,
      repoSlug,
    });
  }

  return items;
}

export function projectNotesToHashtagItems(notes: ProjectNote[]): HashtagItem[] {
  return notes
    .filter((n) => n.title.trim())
    .map((n) => ({
      type: 'project-note' as const,
      id: n.id,
      title: n.title,
      color: '--note-color',
      bgColor: '--note-bg',
    }));
}

// ── Shared rendering ─────────────────────────────────────────────────

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

/** Returns true when `text` contains at least one `#type:id` hashtag token. */
export function hasHashtagTokens(text: string): boolean {
  const re = new RegExp(HASHTAG_TOKEN_RE.source);
  return re.test(text);
}

/**
 * Replace `#type:id` tokens in plain text with inline badge HTML.
 * Plain-text segments are HTML-escaped; badge spans use CSS custom-property
 * colours from `hashtagTypeColors`.
 */
export function renderHashtagTokens(text: string, items: HashtagItem[]): string {
  const itemsByKey = new Map<string, HashtagItem>();
  for (const item of items) {
    itemsByKey.set(`${item.type}:${item.id}`, item);
  }

  const regex = new RegExp(HASHTAG_TOKEN_RE.source, 'g');
  const parts: string[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = regex.exec(text)) !== null) {
    if (match.index > lastIndex) {
      parts.push(escapeHtml(text.slice(lastIndex, match.index)));
    }

    const type = match[1];
    const id = match[2];
    const label = hashtagTypeLabels[type] ?? type;
    const colors = hashtagTypeColors[type] ?? { color: '--text-muted', bg: '--bg-secondary' };
    const item = itemsByKey.get(`${type}:${id}`);
    const title = item
      ? item.title
      : type === 'commit' && id.length > 12
        ? id.slice(0, 8) + '…'
        : id;
    parts.push(
      `<span class="hashtag-badge" style="background: var(${colors.bg}); color: var(${colors.color});">${escapeHtml(label)}: ${escapeHtml(title)}</span>`
    );

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    parts.push(escapeHtml(text.slice(lastIndex)));
  }

  return parts.join('');
}
