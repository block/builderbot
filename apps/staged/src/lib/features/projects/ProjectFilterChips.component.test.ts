// Mounts the real chip bar over the real projectFiltersStore and tagsStore,
// with the backend commands and the other data stores mocked: tag chips list
// the tags in use, in catalog order, with counts; clicking them filters; and a
// deleted tag's filter neither filters nor keeps Clear up.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, TagState } from '../../types';
import { tagState, withTagAssigned } from '../tags/testStubs/tagFixtures';

const { backend, projects } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  projects: [] as Project[],
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
  projectsDataStore: {
    get liveProjects() {
      return projects;
    },
    branchesByProject: new Map(),
    reposByProject: new Map(),
    deletingProjectNames: new Map(),
  },
}));
vi.mock('../../stores/projectState.svelte', () => ({
  projectStateStore: { isUnread: () => false },
}));
vi.mock('../../stores/repoBadges.svelte', () => ({ repoBadgeStore: { lookup: () => null } }));
vi.mock('./projectStatus', () => ({
  getProjectStatus: () => ({ kind: 'idle', runningCount: 0, runActionPhase: null }),
}));
vi.mock('./workspaceLifecycle.svelte', () => ({
  workspaceLifecycle: { getWorktreeErrors: () => new Map() },
}));

import { tagsStore } from '../../stores/tags.svelte';
import ProjectFilterChips from './ProjectFilterChips.svelte';
import { projectFiltersStore } from './projectFilters.svelte';

function project(id: string): Project {
  return {
    id,
    name: `Project ${id}`,
    githubRepo: null,
    location: 'local',
    subpath: null,
    createdAt: 1,
    updatedAt: 1,
  };
}

let component: ReturnType<typeof mount> | null = null;

async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

/** p1: Doing and q4; p2: Doing; p3: P0. */
const initialState = () =>
  tagState([
    { projectId: 'p1', tagId: 'doing' },
    { projectId: 'p1', tagId: 'q4' },
    { projectId: 'p2', tagId: 'doing' },
    { projectId: 'p3', tagId: 'p0' },
  ]);

async function mountChips(state: TagState = initialState()) {
  backend.state = state;
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ProjectFilterChips, { target });
  await settle();
  return target;
}

/** Push a new backend state and let the store refetch it, as on `tags-changed`. */
async function backendChanged(state: TagState) {
  backend.state = state;
  await tagsStore.load({ force: true });
  await settle();
}

/** The chips' labels and counts, Unread and Running first. */
function chips(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLElement>('.filter-chip')].map((el) =>
    (el.textContent ?? '').replace(/\s+/g, ' ').trim()
  );
}

function tagChip(root: ParentNode, name: string): HTMLButtonElement {
  const found = [...root.querySelectorAll<HTMLButtonElement>('.tag-filter')].find(
    (el) => el.querySelector('.tag-filter-name')?.textContent === name
  );
  if (!found) throw new Error(`No tag chip ${name}`);
  return found;
}

function clearChip(root: ParentNode): HTMLButtonElement | null {
  return root.querySelector<HTMLButtonElement>('.clear-chip');
}

const shown = () => projectFiltersStore.filteredProjects.map((p) => p.id);

beforeEach(() => {
  projects.splice(0, projects.length, project('p1'), project('p2'), project('p3'));
});

afterEach(() => {
  projectFiltersStore.clearFilters();
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('ProjectFilterChips tag chips', () => {
  it('lists the tags in use after the status chips, in catalog order, with counts', async () => {
    const root = await mountChips();

    expect(chips(root)).toEqual(['Unread 0', 'Running 0', 'Doing 2', 'P0 1', 'q4 1']);
    // The tooltip names the group, since two groups can share a tag name.
    expect(tagChip(root, 'Doing').title).toBe('Status: Doing');
    expect(tagChip(root, 'q4').title).toBe('q4');
  });

  it('filters by a click, adds with shift-click, and offers Clear', async () => {
    const root = await mountChips();
    expect(clearChip(root)).toBeNull();

    tagChip(root, 'Doing').click();
    await settle();
    expect(tagChip(root, 'Doing').classList.contains('active')).toBe(true);
    expect(shown()).toEqual(['p1', 'p2']);
    expect(clearChip(root)).not.toBeNull();

    tagChip(root, 'q4').dispatchEvent(new MouseEvent('click', { bubbles: true, shiftKey: true }));
    await settle();
    expect(projectFiltersStore.activeFilters).toEqual(new Set(['tag:doing', 'tag:q4']));
    expect(shown()).toEqual(['p1']);

    // A plain click selects just the one clicked.
    tagChip(root, 'P0').click();
    await settle();
    expect(shown()).toEqual(['p3']);

    clearChip(root)!.click();
    await settle();
    expect(shown()).toEqual(['p1', 'p2', 'p3']);
    expect(clearChip(root)).toBeNull();
  });

  it('keeps an active chip at 0 once its last project loses the tag', async () => {
    const root = await mountChips();
    tagChip(root, 'q4').click();
    await settle();

    await backendChanged(withTagAssigned(initialState(), 'p1', 'q4', false));

    expect(tagChip(root, 'q4').textContent).toContain('0');
    expect(tagChip(root, 'q4').classList.contains('active')).toBe(true);
    expect(shown()).toEqual([]);
    tagChip(root, 'q4').click();
    await settle();
    expect(shown()).toEqual(['p1', 'p2', 'p3']);
  });

  it('drops a deleted tag’s filter: its chip goes, it filters nothing, and Clear hides', async () => {
    const root = await mountChips();
    tagChip(root, 'q4').click();
    await settle();
    expect(shown()).toEqual(['p1']);

    const deleted = initialState();
    deleted.tags = deleted.tags.filter((t) => t.id !== 'q4');
    deleted.assignments = deleted.assignments.filter((a) => a.tagId !== 'q4');
    await backendChanged(deleted);

    expect(chips(root)).toEqual(['Unread 0', 'Running 0', 'Doing 2', 'P0 1']);
    expect(shown()).toEqual(['p1', 'p2', 'p3']);
    expect(projectFiltersStore.hasActiveFilters).toBe(false);
    expect(clearChip(root)).toBeNull();
  });
});
