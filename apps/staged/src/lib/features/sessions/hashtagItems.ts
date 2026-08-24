import type { BranchTimeline, HashtagItem, ProjectNote, Branch, ProjectRepo } from '../../types';
import { getBranchTimeline, listProjectNotes } from '../../commands';
import { branchTimelineReadyKey } from '../branches/branchTimelineReady';

/**
 * Regex matching `#type:id` hashtag tokens in plain text. Use with
 * `new RegExp(source, 'g')` for stateful iteration.
 *
 * The id is an allowlist of id characters — alphanumerics and `_`, with
 * interior (never leading/trailing) `-` runs — rather than "anything up to
 * whitespace minus a punctuation denylist". Every real id is a uuid or a hex
 * sha, and notes are cited inline in prose (`… collected in #note:<id>.`), so
 * any character outside that shape is the surrounding sentence, not the id.
 * Unlike a trailing-punctuation denylist, this also keeps out Unicode
 * punctuation that binds with no whitespace (`#note:<id>—still prose`, curly
 * quotes, ellipses) at any position, not just token-final.
 */
export const HASHTAG_TOKEN_RE =
  /#(note|commit|review|project-note|image):([A-Za-z0-9_](?:[A-Za-z0-9_-]*[A-Za-z0-9_])?)/g;

/**
 * Incrementally joins editor content chunks into the raw `value` string,
 * inserting a space between a `#type:id` badge token and immediately
 * following non-whitespace text. Token ids match greedily over id characters
 * (HASHTAG_TOKEN_RE), so `#note:1hello` would swallow the text into the id.
 */
export function createExtractedValueBuilder() {
  let result = '';
  let afterToken = false;

  const push = (text: string, isToken: boolean) => {
    if (!text) return;
    if (afterToken && !/^\s/.test(text)) result += ' ';
    result += text;
    afterToken = isToken;
  };

  return {
    appendText(text: string) {
      push(text, false);
    },
    appendToken(token: string) {
      push(token, true);
    },
    get value() {
      return result;
    },
  };
}

/**
 * Inline SVG markup for each hashtag type icon (lucide icons at 12px).
 *
 * NOTE: These raw SVG strings intentionally duplicate the lucide icon paths
 * used via Lucide Svelte components in the HashtagInput dropdown. This is
 * necessary because `renderHashtagTokens` produces a plain HTML string where
 * Svelte components can't render. If you update an icon here, update the
 * corresponding per-icon `@lucide/svelte/icons/<name>` import in
 * HashtagInput.svelte too.
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

type SortableHashtagItem = HashtagItem & {
  sortTimestamp: number;
  sortOrder: number;
};

type BranchHashtagContext = {
  branchId?: string;
  projectId?: string | null;
  branchName?: string;
  repoSlug?: string;
  repoSubpath?: string | null;
};

const hashtagTypeOrder: Record<HashtagItem['type'], number> = {
  'project-note': 0,
  note: 1,
  commit: 2,
  review: 3,
  image: 4,
};

function sortAndStripHashtagItems(items: SortableHashtagItem[]): HashtagItem[] {
  return [...items].sort(compareHashtagItems).map(stripSortMetadata);
}

function compareHashtagItems(a: SortableHashtagItem, b: SortableHashtagItem): number {
  const sectionDiff = hashtagSectionOrder(a) - hashtagSectionOrder(b);
  if (sectionDiff !== 0) return sectionDiff;

  const timestampDiff = b.sortTimestamp - a.sortTimestamp;
  if (timestampDiff !== 0) return timestampDiff;

  const orderDiff = b.sortOrder - a.sortOrder;
  if (orderDiff !== 0) return orderDiff;

  const typeDiff = hashtagTypeOrder[a.type] - hashtagTypeOrder[b.type];
  if (typeDiff !== 0) return typeDiff;

  return a.title.localeCompare(b.title);
}

function hashtagSectionOrder(item: HashtagItem): number {
  return item.type === 'project-note' ? 0 : 1;
}

function stripSortMetadata(item: SortableHashtagItem): HashtagItem {
  const stripped: HashtagItem = {
    type: item.type,
    id: item.id,
    title: item.title,
    color: item.color,
    bgColor: item.bgColor,
  };

  if (item.subtitle !== undefined) stripped.subtitle = item.subtitle;
  if (item.branchName !== undefined) stripped.branchName = item.branchName;
  if (item.repoSlug !== undefined) stripped.repoSlug = item.repoSlug;
  if (item.repoSubpath !== undefined) stripped.repoSubpath = item.repoSubpath;
  if (item.branchId !== undefined) stripped.branchId = item.branchId;
  if (item.projectId !== undefined) stripped.projectId = item.projectId;
  if (item.noteContent !== undefined) stripped.noteContent = item.noteContent;
  if (item.noteSessionId !== undefined) stripped.noteSessionId = item.noteSessionId;
  if (item.noteUpdatedAt !== undefined) stripped.noteUpdatedAt = item.noteUpdatedAt;
  if (item.imageFilename !== undefined) stripped.imageFilename = item.imageFilename;
  if (item.reviewCommitSha !== undefined) stripped.reviewCommitSha = item.reviewCommitSha;
  if (item.reviewScope !== undefined) stripped.reviewScope = item.reviewScope;

  return stripped;
}

/**
 * Build hashtag items for a single branch scope (+ optional project notes).
 */
export async function buildBranchHashtagItems(
  branchId: string,
  projectId: string | null,
  context: BranchHashtagContext = {}
): Promise<HashtagItem[]> {
  const [timeline, projectNotes] = await Promise.all([
    getBranchTimeline(branchId),
    projectId ? listProjectNotes(projectId) : Promise.resolve([]),
  ]);

  const branchContext = { ...context, branchId, projectId };
  return sortAndStripHashtagItems([
    ...timelineToSortableHashtagItems(timeline, branchContext),
    ...projectNotesToSortableHashtagItems(projectNotes),
  ]);
}

/**
 * Build hashtag items for a project scope (all branches + project notes).
 */
export async function buildProjectHashtagItems(
  projectId: string,
  branches: Branch[],
  reposById?: Map<string, ProjectRepo>,
  knownProjectNotes?: ProjectNote[]
): Promise<HashtagItem[]> {
  const readyBranches = branches.filter((branch) => branchTimelineReadyKey(branch) !== null);

  const [timelineResults, projectNotes] = await Promise.all([
    Promise.allSettled(
      readyBranches.map((b) => getBranchTimeline(b.id).then((t) => ({ branch: b, timeline: t })))
    ),
    knownProjectNotes ? Promise.resolve(knownProjectNotes) : listProjectNotes(projectId),
  ]);

  const timelines = timelineResults
    .filter(
      (r): r is PromiseFulfilledResult<{ branch: Branch; timeline: BranchTimeline }> =>
        r.status === 'fulfilled'
    )
    .map((r) => r.value);

  const items: SortableHashtagItem[] = [];
  for (const { branch, timeline } of timelines) {
    const repo = branch.projectRepoId && reposById ? reposById.get(branch.projectRepoId) : null;
    const repoSlug = repo?.githubRepo;
    const repoSubpath = repo?.subpath;
    items.push(
      ...timelineToSortableHashtagItems(timeline, {
        branchId: branch.id,
        projectId,
        branchName: branch.branchName,
        repoSlug,
        repoSubpath,
      })
    );
  }

  items.push(...projectNotesToSortableHashtagItems(projectNotes));

  return sortAndStripHashtagItems(items);
}

export function timelineToHashtagItems(
  timeline: BranchTimeline,
  branchName?: string,
  repoSlug?: string,
  repoSubpath?: string | null,
  context: Pick<BranchHashtagContext, 'branchId' | 'projectId'> = {}
): HashtagItem[] {
  return sortAndStripHashtagItems(
    timelineToSortableHashtagItems(timeline, { ...context, branchName, repoSlug, repoSubpath })
  );
}

function timelineToSortableHashtagItems(
  timeline: BranchTimeline,
  context: BranchHashtagContext = {}
): SortableHashtagItem[] {
  const items: SortableHashtagItem[] = [];
  const { branchId, projectId, branchName, repoSlug, repoSubpath } = context;

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
      repoSubpath,
      branchId,
      projectId,
      noteContent: note.content,
      noteSessionId: note.sessionId,
      noteUpdatedAt: note.updatedAt,
      sortTimestamp: note.completedAt ?? note.createdAt,
      sortOrder: 0,
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
      repoSubpath,
      branchId,
      projectId,
      sortTimestamp: commit.sortTimestamp,
      sortOrder: commit.order,
    });
  }

  for (const review of timeline.reviews) {
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
      repoSubpath,
      branchId,
      projectId,
      reviewCommitSha: review.commitSha,
      reviewScope: review.scope === 'commit' ? 'commit' : 'branch',
      sortTimestamp: review.completedAt ?? review.createdAt,
      sortOrder: 0,
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
      repoSubpath,
      branchId,
      projectId,
      imageFilename: image.filename,
      sortTimestamp: image.createdAt,
      sortOrder: 0,
    });
  }

  return items;
}

export function projectNotesToHashtagItems(notes: ProjectNote[]): HashtagItem[] {
  return sortAndStripHashtagItems(projectNotesToSortableHashtagItems(notes));
}

function projectNotesToSortableHashtagItems(notes: ProjectNote[]): SortableHashtagItem[] {
  return notes
    .filter((n) => n.title.trim())
    .map((n) => ({
      type: 'project-note' as const,
      id: n.id,
      title: n.title,
      color: '--note-color',
      bgColor: '--note-bg',
      projectId: n.projectId,
      noteContent: n.content,
      noteSessionId: n.sessionId,
      noteUpdatedAt: n.updatedAt,
      sortTimestamp: n.completedAt ?? n.createdAt,
      sortOrder: 0,
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

function hashtagReferenceLookupKeys(type: string, id: string): string[] {
  const exactKey = `${type}:${id}`;
  return type === 'note' ? [exactKey, `project-note:${id}`] : [exactKey];
}

export function findHashtagItemForReference(
  items: readonly HashtagItem[],
  type: string,
  id: string
): HashtagItem | undefined {
  for (const key of hashtagReferenceLookupKeys(type, id)) {
    const item = items.find((candidate) => `${candidate.type}:${candidate.id}` === key);
    if (item) return item;
  }
}

function findHashtagItemInMap(
  itemsByKey: Map<string, HashtagItem>,
  type: string,
  id: string
): HashtagItem | undefined {
  for (const key of hashtagReferenceLookupKeys(type, id)) {
    const item = itemsByKey.get(key);
    if (item) return item;
  }
}

type RenderHashtagTokenOptions = {
  interactive?: boolean;
};

/** Resolve the label for a `#type:id` badge: the item title, else a fallback. */
function hashtagBadgeLabel(type: string, id: string, item: HashtagItem | undefined): string {
  if (item) return item.title;
  return type === 'commit' && id.length > 12 ? id.slice(0, 8) + '…' : id;
}

/**
 * Build the HTML for a single hashtag badge.
 *
 * Colours come from CSS classes (`.hashtag-badge.type-<kind>`) rather than
 * inline `style`, so a badge keeps them in any context that strips `style`.
 * The `data-hashtag-*` attributes let a delegated click handler navigate to the
 * referenced item.
 */
export function renderHashtagBadge(
  type: string,
  id: string,
  item: HashtagItem | undefined,
  options: RenderHashtagTokenOptions = {}
): string {
  const { interactive = true } = options;
  const targetType = item?.type ?? type;
  const iconSvg = hashtagTypeIconSvg[targetType] ?? '';
  const label = hashtagBadgeLabel(type, id, item);
  const ref = `#${type}:${id}`;
  const interactionAttributes = interactive
    ? ` role="button" tabindex="0" data-hashtag-ref="${escapeHtml(ref)}"`
    : '';
  return (
    `<span class="hashtag-badge type-${targetType} stable-raster stable-raster-glyphs"` +
    `${interactionAttributes} data-hashtag-type="${escapeHtml(targetType)}" ` +
    `data-hashtag-id="${escapeHtml(item?.id ?? id)}">${iconSvg} ${escapeHtml(label)}</span>`
  );
}

/**
 * Replace `#type:id` tokens in plain text with inline badge HTML.
 * Plain-text segments are HTML-escaped; badge spans use CSS classes for their
 * colours (see {@link renderHashtagBadge}).
 */
export function renderHashtagTokens(
  text: string,
  items: HashtagItem[],
  options: RenderHashtagTokenOptions = {}
): string {
  const { interactive = true } = options;
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
    parts.push(
      renderHashtagBadge(type, id, findHashtagItemInMap(itemsByKey, type, id), {
        interactive,
      })
    );

    lastIndex = match.index + match[0].length;
  }

  if (lastIndex < text.length) {
    parts.push(escapeHtml(text.slice(lastIndex)));
  }

  return parts.join('');
}
