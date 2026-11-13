// Mounts the real row over the real tagsStore, with the backend commands and
// the data stores mocked: where the remote marker lands, and that a
// project's tags sit on the meta line beside its repo short names.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, ProjectLocation, ProjectRepo, TagState } from '../../types';
import { tagState } from '../tags/testStubs/tagFixtures';
import type { MatchReason } from './projectSearch';

const { backend, rowData } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  rowData: {
    repos: new Map<string, ProjectRepo[]>(),
    /** Short-name badges by `githubRepo`. */
    badges: new Map<string, { shortName: string; hue: number }>(),
  },
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
// The row's status icon and store-derived status are stubbed: these tests
// cover the row's layout, not what the status icon shows.
vi.mock('./ProjectStatusIcon.svelte', () => import('../sessions/testStubs/EmptyStub.svelte'));
vi.mock('./projectStatus', () => ({
  getProjectStatus: () => ({ kind: 'idle', runningCount: 0, runActionPhase: null }),
}));
vi.mock('./workspaceLifecycle.svelte', () => ({
  workspaceLifecycle: { getWorktreeErrors: () => new Map() },
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    liveProjects: [],
    branchesByProject: new Map(),
    deletingProjectNames: new Map(),
    reposByProject: rowData.repos,
    repoCountsByProject: new Map(),
    isProjectHydrated: () => true,
  },
}));
vi.mock('../../stores/projectState.svelte', () => ({
  projectStateStore: { getRunningSessionTypes: () => [] },
}));
vi.mock('../../stores/repoBadges.svelte', () => ({
  repoBadgeStore: { lookup: (githubRepo: string) => rowData.badges.get(githubRepo) ?? null },
}));

import { tagsStore } from '../../stores/tags.svelte';
import ProjectRowContent from './ProjectRowContent.svelte';

const MARKER = '[aria-label="Remote project"]';

let component: ReturnType<typeof mount> | null = null;

function project(id: string, location: ProjectLocation = 'local'): Project {
  return {
    id,
    name: 'Project',
    githubRepo: null,
    location,
    subpath: null,
    createdAt: 1,
    updatedAt: 1,
  } as Project;
}

function repo(projectId: string, githubRepo: string): ProjectRepo {
  return {
    id: `${projectId}:${githubRepo}`,
    projectId,
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

function mountRow(location: ProjectLocation, matchReason: MatchReason | null = null) {
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectRowContent, {
    target,
    props: { project: project('p1', location), matchReason },
  });
  flushSync();
  return target;
}

/** Mount p1's row with the fixture catalog and `tagIds` assigned to it. */
async function mountTaggedRow(tagIds: string[], matchReason: MatchReason | null = null) {
  backend.state = tagState(tagIds.map((tagId) => ({ projectId: 'p1', tagId })));
  await tagsStore.load({ force: true });
  await tick();
  return mountRow('local', matchReason);
}

/** p1 with two repos whose badges have landed: `bb` and `goose`. */
function withRepoBadges() {
  rowData.repos.set('p1', [repo('p1', 'block/goose'), repo('p1', 'block/builderbot')]);
  rowData.badges.set('block/builderbot', { shortName: 'bb', hue: 200 });
  rowData.badges.set('block/goose', { shortName: 'goose', hue: 20 });
}

/** What the meta line's badge cluster holds, in order. */
function clusterTexts(root: ParentNode): string[] {
  return [...root.querySelector('.badge-row')!.children].map((el) => el.textContent!.trim());
}

function chipNames(root: ParentNode): string[] {
  return [...root.querySelectorAll('.tag-chip-name')].map((el) => el.textContent!.trim());
}

beforeEach(async () => {
  rowData.repos.clear();
  rowData.badges.clear();
  backend.state = tagState();
  await tagsStore.load({ force: true });
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('ProjectRowContent remote marker', () => {
  it('renders one cloud inside the row for a remote project', () => {
    const target = mountRow('remote');

    expect(target.querySelectorAll(MARKER)).toHaveLength(1);
    expect(target.querySelector(`.row-main ${MARKER}`)).not.toBeNull();
  });

  it('renders no cloud for a local project', () => {
    const target = mountRow('local');

    expect(target.querySelector('.row-main')).not.toBeNull();
    expect(target.querySelector(MARKER)).toBeNull();
  });

  it('places the cloud after the name and before the meta line', () => {
    const target = mountRow('remote');
    const name = target.querySelector('.project-name')!;
    const marker = target.querySelector(MARKER)!;
    const meta = target.querySelector('.row-meta')!;

    expect(name.compareDocumentPosition(marker) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(marker.compareDocumentPosition(meta) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe('ProjectRowContent tags', () => {
  it('shows the tags as chips after the repo short names, in the same cluster', async () => {
    withRepoBadges();
    const target = await mountTaggedRow(['q4', 'p0']);

    // Repo short names sorted, then the chips: grouped tags, then ungrouped.
    expect(clusterTexts(target)).toEqual(['bb', 'goose', 'P0', 'q4']);
    expect(target.querySelectorAll('.badge-row > .repo-badge')).toHaveLength(2);
    expect(target.querySelectorAll('.badge-row > .tag-chip')).toHaveLength(2);
  });

  it('adds nothing for a project without tags', async () => {
    withRepoBadges();
    const target = await mountTaggedRow([]);

    expect(clusterTexts(target)).toEqual(['bb', 'goose']);
    expect(target.querySelector('.tag-chip')).toBeNull();
    expect(target.querySelector('.tag-chips-more')).toBeNull();
  });

  it('leaves out the icon group’s tag, which the status icon shows', async () => {
    withRepoBadges();
    const target = await mountTaggedRow(['doing', 'p1']);

    expect(chipNames(target)).toEqual(['P1']);
  });

  it('shows two chips, then "+N" naming the rest', async () => {
    withRepoBadges();
    const target = await mountTaggedRow(['p0', 'q4', 'infra']);

    expect(chipNames(target)).toEqual(['P0', 'q4']);
    const more = target.querySelector<HTMLElement>('.badge-row > .tag-chips-more')!;
    expect(more.textContent).toBe('+1');
    expect(more.title).toBe('infra');
    expect(clusterTexts(target)).toEqual(['bb', 'goose', 'P0', 'q4', '+1']);
  });

  it('shows the chips after the repo count while no badge has landed', async () => {
    const target = await mountTaggedRow(['infra']);

    expect(clusterTexts(target)).toEqual(['0 repos', 'infra']);
  });

  it('follows the store when a tag is assigned later', async () => {
    withRepoBadges();
    const target = await mountTaggedRow([]);

    backend.state = tagState([{ projectId: 'p1', tagId: 'q4' }]);
    await tagsStore.load({ force: true });
    await tick();
    flushSync();

    expect(clusterTexts(target)).toEqual(['bb', 'goose', 'q4']);
  });

  it('gives the meta line to a search’s match reason instead', async () => {
    withRepoBadges();
    const reason: MatchReason = { kind: 'branch', label: 'fix-login', more: 0, itemTerms: [] };
    const target = await mountTaggedRow(['q4'], reason);

    expect(target.querySelector('.badge-row')).toBeNull();
    expect(target.querySelector('.tag-chip')).toBeNull();
    expect(target.querySelector('.row-meta')!.textContent).toContain('fix-login');
  });
});
