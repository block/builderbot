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

function timelineToHashtagItems(
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
    const title = review.title || `Review of ${review.commitSha.slice(0, 7)}`;
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

function projectNotesToHashtagItems(notes: ProjectNote[]): HashtagItem[] {
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
