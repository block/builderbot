// Mounts the real header row (group pills, chips and the tag picker) over the
// real tagsStore and tagActions, with the backend commands mocked: the
// assertions are on what the user sees and on which commands a gesture ends
// in. Menus and the picker's popover are the real bits-ui ones.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, Tag, TagState } from '../../types';
import { tagState, withGroupTag, withTagAssigned } from '../tags/testStubs/tagFixtures';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

const { backend, toastError, openSettings } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  toastError: vi.fn(),
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
  createTag: vi.fn(
    async (input: { name: string; groupId?: string | null; assignProjectId?: string | null }) => {
      const created: Tag = {
        id: `new-${input.name}`,
        groupId: input.groupId ?? null,
        name: input.name,
        color: 'blue',
        icon: null,
        position: backend.state.tags.filter((t) => t.groupId === null).length,
        createdAt: 1,
        updatedAt: 1,
      };
      backend.state = {
        ...backend.state,
        tags: [...backend.state.tags, created],
        assignments: input.assignProjectId
          ? [...backend.state.assignments, { projectId: input.assignProjectId, tagId: created.id }]
          : backend.state.assignments,
      };
      return created;
    }
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
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));

import { tagsStore } from '../../stores/tags.svelte';
import ProjectTagPills from './ProjectTagPills.svelte';

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

/** Let bits-ui's floating positioning, focus moves and re-renders land. */
async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

async function mountPills(state: TagState, props: { pickerRequest?: number } = {}) {
  backend.state = state;
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  const reactive = reactiveProps({
    project,
    branches: [],
    disabled: false,
    pickerRequest: props.pickerRequest ?? 0,
  });
  component = mount(ProjectTagPills, { target, props: reactive });
  await settle();
  return { root: target, props: reactive };
}

function pills(root: ParentNode): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>('.tag-group-pill')];
}

function chipNames(root: ParentNode): string[] {
  return [...root.querySelectorAll('.tag-chip-name')].map((el) => el.textContent?.trim() ?? '');
}

async function openMenu(trigger: HTMLElement): Promise<HTMLElement> {
  trigger.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  await settle();
  const menus = document.querySelectorAll<HTMLElement>('[data-slot="dropdown-menu-content"]');
  return menus[menus.length - 1];
}

function menuItem(content: ParentNode, text: string): HTMLElement {
  const item = [...content.querySelectorAll<HTMLElement>('[role^="menuitem"]')].find((el) =>
    el.textContent?.includes(text)
  );
  if (!item) throw new Error(`No menu item "${text}"`);
  return item;
}

function addTagButton(root: ParentNode): HTMLButtonElement {
  return root.querySelector<HTMLButtonElement>('.add-tag-button')!;
}

function pickerInput(): HTMLInputElement | null {
  return document.querySelector<HTMLInputElement>('input[aria-label="Search or create tags"]');
}

async function openPicker(root: ParentNode): Promise<HTMLInputElement> {
  addTagButton(root).click();
  await settle();
  return pickerInput()!;
}

async function type(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  await settle();
}

async function press(input: HTMLInputElement, key: string) {
  input.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }));
  await settle();
}

function activeOption(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[role="option"][data-active="true"]');
}

beforeAll(() => {
  // jsdom lacks the layout APIs the menus' positioning and viewport use.
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

describe('ProjectTagPills', () => {
  it('shows a pill per group in order, the ungrouped tags as chips, then + Tag', async () => {
    const { root } = await mountPills(
      tagState([
        { projectId: 'p1', tagId: 'doing' },
        { projectId: 'p1', tagId: 'infra' },
        { projectId: 'p1', tagId: 'q4' },
        { projectId: 'other', tagId: 'p0' },
      ])
    );

    const [status, priority] = pills(root);
    expect(pills(root)).toHaveLength(2);
    expect(status.textContent).toContain('Doing');
    expect(status.classList.contains('empty')).toBe(false);
    // An empty group is a dim pill with the group's name.
    expect(priority.textContent).toContain('Priority');
    expect(priority.classList.contains('empty')).toBe(true);
    // Ungrouped tags in their position order.
    expect(chipNames(root)).toEqual(['q4', 'infra']);
    expect(addTagButton(root).textContent).toContain('Tag');
  });

  it('puts the icon group’s pill first, as the menus and the board do', async () => {
    const state = tagState([
      { projectId: 'p1', tagId: 'doing' },
      { projectId: 'p1', tagId: 'p0' },
    ]);
    // Make Priority the icon group, though it sorts second.
    state.groups = state.groups.map((g) => ({ ...g, isIconGroup: g.id === 'priority' }));
    const { root } = await mountPills(state);

    expect(pills(root).map((pill) => pill.textContent?.trim())).toEqual(['P0', 'Doing']);
  });

  it('shows the computed status in the icon group pill when it has no tag', async () => {
    const { root } = await mountPills(tagState());

    const status = pills(root)[0];
    expect(status.classList.contains('empty')).toBe(false);
    expect(status.textContent).toContain('No changes yet');

    const menu = await openMenu(status);
    expect(menuItem(menu, 'Default').textContent).toContain('No changes yet');
  });

  it('switches a group’s tag from its pill in one pick', async () => {
    const { root } = await mountPills(tagState([{ projectId: 'p1', tagId: 'p0' }]));

    const menu = await openMenu(pills(root)[1]);
    expect(menuItem(menu, 'None')).toBeTruthy();
    menuItem(menu, 'P1').click();
    await settle();

    expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'priority', 'p1');
    expect(pills(root)[1].textContent).toContain('P1');
  });

  it('opens Settings › Tags from Edit tags…', async () => {
    const { root } = await mountPills(tagState());

    menuItem(await openMenu(pills(root)[1]), 'Edit tags…').click();
    await settle();

    expect(openSettings).toHaveBeenCalledWith('tags');
  });

  it('removes an ungrouped tag from its chip', async () => {
    const { root } = await mountPills(tagState([{ projectId: 'p1', tagId: 'q4' }]));

    root.querySelector<HTMLButtonElement>('[aria-label="Remove tag q4"]')!.click();
    await settle();

    expect(commandsMock.setProjectTagAssigned).toHaveBeenCalledWith('p1', 'q4', false);
    expect(chipNames(root)).toEqual([]);
  });

  describe('the tag picker', () => {
    it('creates and assigns an ungrouped tag on Enter when nothing has the name', async () => {
      const { root } = await mountPills(tagState());
      const input = await openPicker(root);

      await type(input, 'q4-launch');
      expect(activeOption()?.textContent).toContain('Create tag “q4-launch”');
      await press(input, 'Enter');

      expect(commandsMock.createTag).toHaveBeenCalledWith({
        name: 'q4-launch',
        groupId: null,
        assignProjectId: 'p1',
      });
      expect(chipNames(root)).toEqual(['q4-launch']);
      // The search clears and the new tag shows ticked, ready for the next.
      expect(input.value).toBe('');
      expect(
        document.querySelector('[data-tag-option="new-q4-launch"]')?.getAttribute('aria-checked')
      ).toBe('true');
    });

    it('toggles an existing tag that the query names, rather than creating it', async () => {
      const { root } = await mountPills(tagState());
      const input = await openPicker(root);

      await type(input, 'INFRA');
      expect(document.body.textContent).not.toContain('Create tag');
      await press(input, 'Enter');

      expect(commandsMock.createTag).not.toHaveBeenCalled();
      expect(commandsMock.setProjectTagAssigned).toHaveBeenCalledWith('p1', 'infra', true);
      expect(chipNames(root)).toEqual(['infra']);
    });

    it('picks a partial match on Enter, with the create row one step below', async () => {
      const { root } = await mountPills(tagState());
      const input = await openPicker(root);

      await type(input, 'inf');
      expect(activeOption()?.textContent).toContain('infra');
      await press(input, 'Enter');

      expect(commandsMock.createTag).not.toHaveBeenCalled();
      expect(commandsMock.setProjectTagAssigned).toHaveBeenCalledWith('p1', 'infra', true);
      expect(chipNames(root)).toEqual(['infra']);

      // The query stays, and creating the near-duplicate is one ArrowDown away.
      await press(input, 'ArrowDown');
      expect(activeOption()?.textContent).toContain('Create tag “inf”');
    });

    it('moves with the arrow keys and picks a group’s tag like a radio', async () => {
      const { root } = await mountPills(tagState([{ projectId: 'p1', tagId: 'idea' }]));
      const input = await openPicker(root);

      expect(activeOption()?.textContent).toContain('Idea');
      await press(input, 'ArrowDown');
      expect(activeOption()?.textContent).toContain('Doing');
      await press(input, 'Enter');

      expect(commandsMock.setProjectGroupTag).toHaveBeenCalledWith('p1', 'status', 'doing');
      expect(pills(root)[0].textContent).toContain('Doing');

      // Picking the checked one again clears the group.
      await press(input, 'Enter');
      expect(commandsMock.setProjectGroupTag).toHaveBeenLastCalledWith('p1', 'status', null);
    });

    it('wraps from the top to Manage tags…, which opens Settings › Tags', async () => {
      const { root } = await mountPills(tagState());
      const input = await openPicker(root);

      await press(input, 'ArrowUp');
      expect(activeOption()?.textContent).toContain('Manage tags…');
      await press(input, 'Enter');

      expect(openSettings).toHaveBeenCalledWith('tags');
      // The popover unmounts through bits-ui's presence layer, which can land
      // a frame after the settle rounds on a loaded machine.
      await vi.waitFor(() => expect(pickerInput()).toBeNull());
    });

    it('opens when the T shortcut bumps the request, but not on mount', async () => {
      const { props } = await mountPills(tagState(), { pickerRequest: 3 });
      expect(pickerInput()).toBeNull();

      props.pickerRequest = 4;
      await settle();
      expect(pickerInput()).not.toBeNull();
    });
  });
});
