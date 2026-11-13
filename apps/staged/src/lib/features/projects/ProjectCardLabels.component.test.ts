// Mounts a card's labels (the home grid's and the board's) over the real
// tagsStore, with the backend commands and the badge store mocked: the
// project's repos show under their badges' short names (the full path in the
// tooltip, and in place of a repo with no badge), its tags sit as chips in
// the same cluster, and a board card leaves out the chip its column already
// stands for.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectRepo, TagState } from '../../types';
import { tag, tagState } from '../tags/testStubs/tagFixtures';

const { backend, badges } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  /** Badges by `githubRepo`, or `githubRepo:subpath` for a subpath's. */
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
  repoBadgeStore: {
    lookup: (githubRepo: string, subpath: string | null | undefined) =>
      badges.get(subpath ? `${githubRepo}:${subpath}` : githubRepo) ?? null,
  },
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

function repo(githubRepo: string, overrides: Partial<ProjectRepo> = {}): ProjectRepo {
  return {
    id: `${githubRepo}:${overrides.subpath ?? ''}`,
    projectId: 'p1',
    githubRepo,
    branchName: 'main',
    subpath: null,
    isPrimary: false,
    reason: null,
    headRepo: null,
    createdAt: 1,
    updatedAt: 1,
    ...overrides,
  };
}

/** One repo with a badge, one without. */
const REPOS = [repo('block/goose'), repo('block/builderbot')];

/** Mount p1's labels with `tagIds` assigned to it in `state`'s catalog. */
async function mountLabels(
  tagIds: string[],
  {
    project = PROJECT,
    repos = REPOS,
    excludeTagGroupId = null,
    state = tagState(),
  }: {
    project?: Project;
    repos?: ProjectRepo[];
    excludeTagGroupId?: string | null;
    state?: TagState;
  } = {}
) {
  badges.clear();
  badges.set('block/builderbot', { shortName: 'bb', hue: 200 });
  badges.set('block/builderbot:apps/staged', { shortName: 'staged', hue: 120 });
  backend.state = { ...state, assignments: tagIds.map((tagId) => ({ projectId: 'p1', tagId })) };
  await tagsStore.load({ force: true });
  await tick();
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectCardLabels, {
    target,
    props: { project, repos, excludeTagGroupId },
  });
  flushSync();
  return target;
}

/** What the cluster holds, in order. */
function clusterTexts(root: ParentNode): string[] {
  return [...root.querySelector('.card-labels')!.children].map((el) => el.textContent!.trim());
}

/** The repos' labels, in order: what each shows and its tooltip. */
function repoLabels(root: ParentNode): { text: string; title: string }[] {
  return [
    ...root.querySelectorAll<HTMLElement>('.card-labels > .repo-badge-label, .repo-line'),
  ].map((el) => ({
    text: el.textContent!.trim(),
    // A badge carries the tooltip itself; a plain label's is its RepoLabel's.
    title: el.title || el.querySelector<HTMLElement>('.repo-label')!.title,
  }));
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

    // Repos sorted by full path (the badged one under its short name), then
    // the chips: grouped tags, then ungrouped.
    expect(clusterTexts(target)).toEqual(['bb', 'block/goose', 'P0', 'infra']);
    expect(target.querySelectorAll('.card-labels > .repo-badge-label')).toHaveLength(1);
    expect(target.querySelectorAll('.card-labels > .repo-line')).toHaveLength(1);
    expect(target.querySelectorAll('.card-labels > .tag-chip')).toHaveLength(2);
  });

  it('adds nothing for a project without tags', async () => {
    const target = await mountLabels([]);

    expect(clusterTexts(target)).toEqual(['bb', 'block/goose']);
    expect(target.querySelector('.tag-chip')).toBeNull();
    expect(target.querySelector('.tag-chips-more')).toBeNull();
  });

  it('shows a badged repo’s short name, keeping the full path as its tooltip', async () => {
    const target = await mountLabels([]);

    expect(repoLabels(target)).toEqual([
      { text: 'bb', title: 'block/builderbot' },
      // No badge, so no short name: the full path stays.
      { text: 'block/goose', title: 'block/goose' },
    ]);
  });

  it('shows a monorepo subpath under its own short name', async () => {
    const target = await mountLabels([], {
      repos: [
        repo('block/builderbot', { subpath: 'apps/staged' }),
        repo('block/builderbot'),
        // A subpath without a badge keeps its full path.
        repo('block/builderbot', { subpath: 'apps/web' }),
      ],
    });

    // Still ordered by full path, whatever the short names are.
    expect(repoLabels(target)).toEqual([
      { text: 'bb', title: 'block/builderbot' },
      { text: 'staged', title: 'block/builderbot/apps/staged' },
      { text: 'block/builderbot/apps/web', title: 'block/builderbot/apps/web' },
    ]);
  });

  it('names a fork-backed repo by its badge, with the fork’s path as the tooltip', async () => {
    const target = await mountLabels([], {
      repos: [repo('block/builderbot', { headRepo: 'someone/builderbot' })],
    });

    expect(repoLabels(target)).toEqual([{ text: 'bb', title: 'someone/builderbot' }]);
  });

  it('shortens the project’s own repo while its repos haven’t loaded', async () => {
    const target = await mountLabels([], {
      project: { ...PROJECT, githubRepo: 'block/builderbot', subpath: 'apps/staged' },
      repos: [],
    });

    expect(repoLabels(target)).toEqual([{ text: 'staged', title: 'block/builderbot/apps/staged' }]);
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
