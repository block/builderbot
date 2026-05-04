import type { BranchTimeline, HashtagItem, ProjectNote, Branch, ProjectRepo } from '../../types';
import { getBranchTimeline, listProjectNotes } from '../../commands';

/** Regex matching `#type:id` hashtag tokens in plain text. Use with `new RegExp(source, 'g')` for stateful iteration. */
export const HASHTAG_TOKEN_RE = /#(note|commit|review|project-note|image):([^\s]+)/g;

/**
 * Inline SVG markup for each hashtag type icon (lucide icons at 12px).
 *
 * NOTE: These raw SVG strings intentionally duplicate the lucide icon paths
 * used via `lucide-svelte` components in the HashtagInput dropdown. This is
 * necessary because `renderHashtagTokens` produces a plain HTML string where
 * Svelte components can't render. If you update an icon here, update the
 * corresponding lucide-svelte component import in HashtagInput.svelte too.
 */
export const hashtagTypeIconSvg: Record<string, string> = {
  note: '<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 13H8"/><path d="M16 13h-2"/><path d="M10 17H8"/><path d="M16 17h-2"/></svg>',
  commit:
    '<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v6"/><circle cx="12" cy="12" r="3"/><path d="M12 15v6"/></svg>',
  review:
    '<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M4.268 21a2 2 0 0 0 1.727 1H18a2 2 0 0 0 2-2V7l-5-5H6a2 2 0 0 0-2 2v3"/><path d="m9 18-1.5-1.5"/><circle cx="5" cy="14" r="3"/></svg>',
  'project-note':
    '<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 13H8"/><path d="M16 13h-2"/><path d="M10 17H8"/><path d="M16 17h-2"/></svg>',
  image:
    '<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="18" height="18" x="3" y="3" rx="2" ry="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/></svg>',
};

/** CSS custom-property names for each hashtag type's foreground and background colors. */
export const hashtagTypeColors: Record<string, { color: string; bg: string }> = {
  note: { color: '--note-color', bg: '--note-bg' },
  commit: { color: '--commit-color', bg: '--commit-bg' },
  review: { color: '--review-color', bg: '--review-bg' },
  'project-note': { color: '--note-color', bg: '--note-bg' },
  image: { color: '--image-color', bg: '--image-bg' },
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
    if (note.completedAt == null) continue;
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
    if (!commit.sha) continue;
    items.push({
      type: 'commit',
      id: commit.sha,
      title: commit.subject,
      color: '--commit-color',
      bgColor: '--commit-bg',
      branchName,
      repoSlug,
    });
  }

  for (const review of timeline.reviews) {
    if (review.isAuto) continue;
    if (review.completedAt == null) continue;
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

  for (const image of timeline.images) {
    if (!image.filename.trim()) continue;
    items.push({
      type: 'image',
      id: image.id,
      title: image.filename,
      color: '--image-color',
      bgColor: '--image-bg',
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

export function escapeHtml(text: string): string {
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
    const iconSvg = hashtagTypeIconSvg[type] ?? '';
    const colors = hashtagTypeColors[type] ?? { color: '--text-muted', bg: '--bg-secondary' };
    const item = itemsByKey.get(`${type}:${id}`);
    const title = item
      ? item.title
      : type === 'commit' && id.length > 12
        ? id.slice(0, 8) + '…'
        : id;
    parts.push(
      `<span class="hashtag-badge" style="background: var(${colors.bg}); color: var(${colors.color});">${iconSvg} ${escapeHtml(title)}</span>`
    );

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    parts.push(escapeHtml(text.slice(lastIndex)));
  }

  return parts.join('');
}
