// Mounts the context menu's tag items inside a real bits-ui context menu,
// over the real tagsStore and tagActions with the backend commands mocked:
// a radio pick in a group's submenu ends in `set_project_group_tag`, a
// checkbox in Tags in `set_project_tag_assigned`.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, TagState } from '../../types';
import { tagState, withGroupTag, withTagAssigned } from '../tags/testStubs/tagFixtures';

const { backend, openSettings } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  openSettings: vi.fn(),
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
  setProjectGroupTag: vi.fn(
    async (_projectId: string, _groupId: string, _tagId: string | null) => {}
  ),
  setProjectTagAssigned: vi.fn(
    async (_projectId: string, _tagId: string, _assigned: boolean) => {}
  ),
}));

vi.mock('../../commands', () => commandsMock);
vi.mock('../../api/commands', () => commandsMock);
vi.mock('../../transport', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../transport')>()),
  listenToEvent: () => () => {},
}));
vi.mock('../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    liveProjects: [{ id: 'p1' }],
    branchesByProject: new Map(),
    isProjectDeleting: () => false,
    isProjectHydrated: () => true,
  },
}));
vi.mock('../layout/navigation.svelte', () => ({ openSettings }));
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));

import { tagsStore } from '../../stores/tags.svelte';
import { newTagDialog } from '../tags/newTagRequest.svelte';
import Harness from './testStubs/ProjectTagMenuItemsHarness.svelte';

const project: Project = {
  id: 'p1',
  name: 'Project',
  githubRepo: null,
  location: 'local',
  subpath: null,
  createdAt: 1,
  updatedAt: 1,
};

let component: ReturnType<typeof mount> | null = null;

async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

/** Mount the harness and open its context menu. */
async function openContextMenu(state: TagState): Promise<HTMLElement> {
  backend.state = state;
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(Harness, { target, props: { project } });
  await settle();
  target
    .querySelector('.harness-trigger')!
    .dispatchEvent(
      new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 10, clientY: 10 })
    );
  await settle();
  return document.querySelector<HTMLElement>('[data-slot="context-menu-content"]')!;
}

function subTriggers(menu: ParentNode): HTMLElement[] {
  return [...menu.querySelectorAll<HTMLElement>('[data-tag-group-trigger]')];
}

/** Open a submenu by its trigger and return its content. */
async function openSub(menu: ParentNode, key: string): Promise<HTMLElement> {
  const trigger = menu.querySelector<HTMLElement>(`[data-tag-group-trigger="${key}"]`)!;
  trigger.focus();
  trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
  await settle();
  const subs = document.querySelectorAll<HTMLElement>('[data-slot="context-menu-sub-content"]');
  return subs[subs.length - 1];
}

function item(content: ParentNode, text: string): HTMLElement {
  const found = [...content.querySelectorAll<HTMLElement>('[role^="menuitem"]')].find((el) =>
    el.textContent?.includes(text)
  );
  if (!found) throw new Error(`No menu item "${text}"`);
  return found;
}

beforeAll(() => {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

afterAll(() => {
  delete (window as Partial<Window>).matchMedia;
  delete (globalThis as Partial<typeof globalThis>).ResizeObserver;
});

beforeEach(() => {
  vi.clearAllMocks();
  newTagDialog.open = false;
  // The writes change what the next get_tag_state returns, as the backend's would.
  commandsMock.setProjectGroupTag.mockImplementation(async (projectId, groupId, tagId) => {
    backend.state = withGroupTag(backend.state, projectId, groupId, tagId);
  });
  commandsMock.setProjectTagAssigned.mockImplementation(async (projectId, tagId, assigned) => {
    backend.state = withTagAssigned(backend.state, projectId, tagId, assigned);
  });
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('ProjectTagMenuItems', () => {
  it('lists the icon group first, then the other groups, then Tags, with current values', async () => {
    const state = tagState([
      { projectId: 'p1', tagId: 'p0' },
      { projectId: 'p1', tagId: 'q4' },
    ]);
    // Make Priority the icon group, though it sorts second.
    state.groups = state.groups.map((g) => ({ ...g, isIconGroup: g.id === 'priority' }));
    const menu = await openContextMenu(state);

    const triggers = subTriggers(menu);
    expect(triggers.map((t) => t.dataset.tagGroupTrigger)).toEqual([
      'priority',
      'status',
      'ungrouped',
    ]);
    expect(triggers[0].textContent).toContain('P0');
    expect(triggers[1].textContent).toContain('None');
    // Tags counts the ungrouped tags the project carries.
    expect(triggers[2].textContent).toContain('1');
  });

  it('sets a group’s tag from its radio submenu', async () => {
    const menu = await openContextMenu(tagState([{ projectId: 'p1', tagId: 'p0' }]));
    const sub = await openSub(menu, 'priority');

    expect(item(sub, 'P0').getAttribute('aria-checked')).toBe('true');
    item(sub, 'P1').click();
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'priority', 'p1');
    expect(tagsStore.groupTagFor('p1', 'priority')?.id).toBe('p1');
  });

  it('clears the icon group with Default, which previews the computed status', async () => {
    const menu = await openContextMenu(tagState([{ projectId: 'p1', tagId: 'doing' }]));
    const sub = await openSub(menu, 'status');

    const fallback = item(sub, 'Default');
    expect(fallback.textContent).toContain('No changes yet');
    fallback.click();
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', null);
  });

  it('adds and removes ungrouped tags from the Tags checkboxes', async () => {
    const menu = await openContextMenu(tagState([{ projectId: 'p1', tagId: 'infra' }]));
    const sub = await openSub(menu, 'ungrouped');

    const infra = item(sub, 'infra');
    expect(infra.getAttribute('aria-checked')).toBe('true');
    item(sub, 'q4').click();
    await settle();
    infra.click();
    await settle();

    expect(commandsMock.setProjectTagAssigned).toHaveBeenCalledWith('p1', 'q4', true);
    expect(commandsMock.setProjectTagAssigned).toHaveBeenCalledWith('p1', 'infra', false);
    expect(commandsMock.setProjectGroupTag).not.toHaveBeenCalled();
    expect([...tagsStore.tagIdsFor('p1')]).toEqual(['q4']);
  });

  it('asks for a new tag for this project, and links to Settings › Tags', async () => {
    let menu = await openContextMenu(tagState());
    item(await openSub(menu, 'ungrouped'), 'New tag…').click();
    await settle();

    expect(newTagDialog.open).toBe(true);
    expect(newTagDialog.project?.id).toBe('p1');

    unmount(component!);
    component = null;
    document.body.innerHTML = '';
    menu = await openContextMenu(tagState());
    item(await openSub(menu, 'ungrouped'), 'Manage tags…').click();
    await settle();

    expect(openSettings).toHaveBeenCalledWith('tags');
  });
});
