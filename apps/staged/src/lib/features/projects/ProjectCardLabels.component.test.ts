// Mounts a card's labels (the home grid's and the board's) over the real
// tagsStore, with the backend commands and the badge store mocked: the
// project's tags sit as chips in the same cluster as its repos, and a board
// card leaves out the chip its column already stands for.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectRepo, TagState } from '../../types';
import { tag, tagState } from '../tags/testStubs/tagFixtures';

const { backend, badges } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  /** Badges by `githubRepo`. */
  badges: new Map<string, { shortName: string; hue: number }>(),
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
}));

vi.mock('../../commands', () => commandsMock);
vi.mock('../../api/commands', () => commandsMock);
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: { liveProjects: [] },
}));
vi.mock('../../stores/repoBadges.svelte', () => ({
  repoBadgeStore: { lookup: (githubRepo: string) => badges.get(githubRepo) ?? null },
}));

import { tagsStore } from '../../stores/tags.svelte';
import ProjectCardLabels from './ProjectCardLabels.svelte';

let component: ReturnType<typeof mount> | null = null;

const PROJECT: Project = {
  id: 'p1',
  name: 'Project',
  githubRepo: null,
  location: 'local',
  subpath: null,
  createdAt: 1,
  updatedAt: 1,
};

function repo(githubRepo: string): ProjectRepo {
  return {
    id: githubRepo,
    projectId: 'p1',
    githubRepo,
    branchName: 'main',
    subpath: null,
    isPrimary: false,
    reason: null,
    headRepo: null,
    createdAt: 1,
    updatedAt: 1,
  };
}

/** One repo with a badge, one without. */
const REPOS = [repo('block/goose'), repo('block/builderbot')];

/** Mount p1's labels with `tagIds` assigned to it in `state`'s catalog. */
async function mountLabels(
  tagIds: string[],
  {
    repos = REPOS,
    excludeTagGroupId = null,
    state = tagState(),
  }: { repos?: ProjectRepo[]; excludeTagGroupId?: string | null; state?: TagState } = {}
) {
  badges.clear();
  badges.set('block/builderbot', { shortName: 'bb', hue: 200 });
  backend.state = { ...state, assignments: tagIds.map((tagId) => ({ projectId: 'p1', tagId })) };
  await tagsStore.load({ force: true });
  await tick();
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectCardLabels, {
    target,
    props: { project: PROJECT, repos, excludeTagGroupId },
  });
  flushSync();
  return target;
}

/** What the cluster holds, in order. */
function clusterTexts(root: ParentNode): string[] {
  return [...root.querySelector('.card-labels')!.children].map((el) => el.textContent!.trim());
}

function chipNames(root: ParentNode): string[] {
  return [...root.querySelectorAll('.tag-chip-name')].map((el) => el.textContent!.trim());
}

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('ProjectCardLabels', () => {
  it('shows the tags as chips after the repos, in the same cluster', async () => {
    const target = await mountLabels(['infra', 'p0']);

    // Repos sorted, then the chips: grouped tags, then ungrouped.
    expect(clusterTexts(target)).toEqual(['block/builderbot', 'block/goose', 'P0', 'infra']);
    expect(target.querySelectorAll('.card-labels > .repo-badge-label')).toHaveLength(1);
    expect(target.querySelectorAll('.card-labels > .repo-line')).toHaveLength(1);
    expect(target.querySelectorAll('.card-labels > .tag-chip')).toHaveLength(2);
  });

  it('adds nothing for a project without tags', async () => {
    const target = await mountLabels([]);

    expect(clusterTexts(target)).toEqual(['block/builderbot', 'block/goose']);
    expect(target.querySelector('.tag-chip')).toBeNull();
    expect(target.querySelector('.tag-chips-more')).toBeNull();
  });

  it('shows the chips beside "No repo attached" for a project with no repo', async () => {
    const target = await mountLabels(['q4'], { repos: [] });

    expect(target.querySelector('.card-labels')!.textContent).toContain('No repo attached');
    expect(chipNames(target)).toEqual(['q4']);
  });

  it('leaves out the icon group’s tag, which the status icon shows', async () => {
    const target = await mountLabels(['doing', 'p1', 'q4']);

    expect(chipNames(target)).toEqual(['P1', 'q4']);
  });

  it('shows three chips, then "+N" naming the rest', async () => {
    const base = tagState();
    const state = {
      ...base,
      tags: [...base.tags, tag('web', null, 2), tag('mobile', null, 3)],
    };
    const target = await mountLabels(['p0', 'q4', 'infra', 'web', 'mobile'], { state });

    expect(chipNames(target)).toEqual(['P0', 'q4', 'infra']);
    const more = target.querySelector<HTMLElement>('.card-labels > .tag-chips-more')!;
    expect(more.textContent).toBe('+2');
    expect(more.title).toBe('web, mobile');
  });

  it('leaves out the excluded group’s tag and keeps the rest (a board card)', async () => {
    const target = await mountLabels(['p0', 'q4', 'infra'], { excludeTagGroupId: 'priority' });

    expect(chipNames(target)).toEqual(['q4', 'infra']);
    expect(target.querySelector('.tag-chips-more')).toBeNull();
  });
});
