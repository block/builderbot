// Mounts the pinned-board rows over the real tagsStore with the backend
// commands mocked: one row per pinned group in group order, the active one
// marked, a click selecting the group, and rows following a catalog echo.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { TagState } from '../../types';
import { tagState } from '../tags/testStubs/tagFixtures';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

const { backend } = vi.hoisted(() => ({ backend: { state: null as unknown as TagState } }));

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

import { tagsStore } from '../../stores/tags.svelte';
import SidebarBoardRows from './SidebarBoardRows.svelte';

let component: ReturnType<typeof mount> | null = null;

async function settle() {
  for (let i = 0; i < 2; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

/** Status and Priority pinned, in that order; a third, unpinned, group too. */
function pinned(): TagState {
  const state = tagState();
  return {
    ...state,
    groups: [
      ...state.groups.map((g) => ({ ...g, showInSidebar: true })),
      { ...state.groups[0], id: 'effort', name: 'Effort', position: 2, isIconGroup: false },
    ],
  };
}

async function mountRows(state: TagState, activeGroupId: string | null = null) {
  backend.state = state;
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  const onSelect = vi.fn();
  const props = reactiveProps({ activeGroupId, onSelect });
  component = mount(SidebarBoardRows, { target, props });
  await settle();
  return { target, props, onSelect };
}

function rows(root: ParentNode): HTMLButtonElement[] {
  return [...root.querySelectorAll<HTMLButtonElement>('[data-board-group]')];
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('SidebarBoardRows', () => {
  it('lists the pinned groups in group order, and nothing when none is pinned', async () => {
    const { target } = await mountRows(pinned());
    expect(rows(target).map((row) => row.dataset.boardGroup)).toEqual(['status', 'priority']);
    expect(rows(target).map((row) => row.textContent?.trim())).toEqual(['Status', 'Priority']);

    backend.state = tagState();
    await tagsStore.load({ force: true });
    await settle();
    expect(rows(target)).toHaveLength(0);
    expect(target.querySelector('.board-rows')).toBeNull();
  });

  it('marks the row whose board the layer shows', async () => {
    const { target, props } = await mountRows(pinned(), 'priority');
    const [status, priority] = rows(target);
    expect(priority.classList.contains('active')).toBe(true);
    expect(priority.getAttribute('aria-current')).toBe('page');
    expect(status.classList.contains('active')).toBe(false);

    props.activeGroupId = null;
    await settle();
    expect(rows(target).some((row) => row.classList.contains('active'))).toBe(false);
  });

  it('selects the group on click', async () => {
    const { target, onSelect } = await mountRows(pinned());
    rows(target)[1].click();
    expect(onSelect).toHaveBeenCalledWith('priority');
  });

  it('follows a catalog echo: an unpinned group loses its row, a renamed one keeps it', async () => {
    const { target } = await mountRows(pinned());
    const state = pinned();
    backend.state = {
      ...state,
      groups: state.groups.map((g) =>
        g.id === 'status'
          ? { ...g, showInSidebar: false }
          : g.id === 'priority'
            ? { ...g, name: 'Prio' }
            : g
      ),
    };
    await tagsStore.load({ force: true });
    await settle();
    expect(rows(target).map((row) => row.textContent?.trim())).toEqual(['Prio']);
  });
});
