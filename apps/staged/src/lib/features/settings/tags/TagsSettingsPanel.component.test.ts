// Mounts the real panel over the real tagsStore and tagActions, with the
// backend commands mocked: the assertions are on what the user sees and on
// which commands a gesture ends in. Menus and dialogs are the real bits-ui
// ones, opened and clicked the way a user would.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DeleteTagGroupMode, NewTag, TagColor, TagState } from '../../../types';
import {
  withGroupOrder,
  withTagMoved,
  withTagValue,
  withoutGroup,
  withoutTag,
} from '../../tags/tagCatalogPatches';
import { group, scopeIds, tagState } from '../../tags/testStubs/tagFixtures';

const { backend, toastError } = vi.hoisted(() => ({
  backend: { state: null as unknown as TagState },
  toastError: vi.fn(),
}));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({
    data: structuredClone(backend.state),
    revalidating: null,
  })),
  createTagGroup: vi.fn(async (name: string, _tags: NewTag[]) => {
    const group = {
      id: `group-${name}`,
      name,
      position: backend.state.groups.length,
      isIconGroup: false,
      showInSidebar: false,
      createdAt: 1,
      updatedAt: 1,
    };
    backend.state = { ...backend.state, groups: [...backend.state.groups, group] };
    return group;
  }),
  deleteTagGroup: vi.fn(async (_groupId: string, _mode: DeleteTagGroupMode) => {}),
  moveTag: vi.fn(async (_tagId: string, _groupId: string | null, _index: number) => {}),
  createTag: vi.fn(),
  updateTag: vi.fn(
    async (_tagId: string, _value: { name: string; color: TagColor; icon: string | null }) => {}
  ),
  deleteTag: vi.fn(async (_tagId: string) => {}),
  renameTagGroup: vi.fn(async () => {}),
  setIconTagGroup: vi.fn(async () => {}),
  setTagGroupSidebar: vi.fn(async (_groupId: string, _shown: boolean) => {}),
  reorderTagGroups: vi.fn(async (_groupIds: string[]) => {}),
}));

vi.mock('../../../commands', () => commandsMock);
vi.mock('../../../api/commands', () => commandsMock);
vi.mock('../../../transport', () => ({ listenToEvent: () => () => {} }));
vi.mock('../../../stores/projectsData.svelte', () => ({
  projectsDataStore: {
    liveProjects: [{ id: 'a' }, { id: 'b' }, { id: 'c' }],
    isProjectDeleting: () => false,
  },
}));
vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }));

import { POINTER_DRAG_ATTRIBUTE } from '../../../shared/pointerDrag';
import { tagsStore } from '../../../stores/tags.svelte';
import { TAG_DRAG_TYPE } from './tagSettingsDrag';
import TagsSettingsPanel from './TagsSettingsPanel.svelte';

let component: ReturnType<typeof mount> | null = null;

/** Let bits-ui's floating positioning, focus moves and re-renders land. */
async function settle() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

async function mountPanel(state: TagState): Promise<HTMLElement> {
  backend.state = state;
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(TagsSettingsPanel, { target });
  await settle();
  return target;
}

function nameInputs(root: ParentNode, label: string): string[] {
  return [...root.querySelectorAll<HTMLInputElement>(`input[aria-label="${label}"]`)].map(
    (input) => input.value
  );
}

function row(tagId: string): HTMLElement {
  return document.querySelector<HTMLElement>(`[data-tag-row="${tagId}"]`)!;
}

/** Open a menu from its trigger and return its content. */
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

function dialog(): HTMLElement {
  return document.querySelector<HTMLElement>('[role="alertdialog"]')!;
}

function button(root: ParentNode, text: string): HTMLButtonElement {
  const found = [...root.querySelectorAll<HTMLButtonElement>('button')].find(
    (el) => el.textContent?.trim() === text
  );
  if (!found) throw new Error(`No button "${text}"`);
  return found;
}

/**
 * Dispatch a drag event on `el` and return it, for a tag's drag, which is an
 * HTML5 one. jsdom lays nothing out, so every rect is empty at the origin:
 * the default `clientY` is the lower half of whatever is under it, and a
 * negative one the upper half.
 *
 * jsdom has no DataTransfer either. Without `data` the event carries none,
 * as from an engine that drops what a drag carries. With it, the event
 * carries a stub holding those types.
 */
function drag(
  eventName: string,
  el: HTMLElement,
  clientY = 10,
  data?: Record<string, string>
): MouseEvent {
  const event = new MouseEvent(eventName, { bubbles: true, cancelable: true, clientY });
  if (data) {
    const store = new Map(Object.entries(data));
    Object.defineProperty(event, 'dataTransfer', {
      value: {
        effectAllowed: 'all',
        dropEffect: 'none',
        get types() {
          return [...store.keys()];
        },
        setData: (key: string, value: string) => void store.set(key, value),
        getData: (key: string) => store.get(key) ?? '',
        setDragImage: () => {},
      },
    });
  }
  el.dispatchEvent(event);
  return event;
}

/** The stub DataTransfer of an event `drag` sent with `data`. */
function carried(event: Event): DataTransfer {
  return (event as DragEvent).dataTransfer!;
}

/** A group's section, the drop zone for a dragged group. */
function section(groupId: string): HTMLElement {
  return document.querySelector<HTMLElement>(`[data-tag-group="${groupId}"]`)!;
}

function groupHandle(groupId: string): HTMLElement {
  return section(groupId).querySelector<HTMLElement>('.group-head .drag-handle')!;
}

interface Point {
  clientX: number;
  clientY: number;
}

/**
 * Lay the list of sections out, which jsdom does not: a 600 by 500 list at
 * the origin, with 560px sections 120px tall and 24px apart from (16, 16),
 * in the order they are in when asked. So the first spans 16 to 136 down,
 * the second 160 to 280 and a third 304 to 424. A group's drag is made of
 * pointer events, and the panel resolves each point from these rects.
 */
function layOutGroups(root: ParentNode): HTMLElement {
  const list = root.querySelector<HTMLElement>('.panel-body')!;
  const rect = (left: number, top: number, width: number, height: number) =>
    ({ left, right: left + width, top, bottom: top + height, width, height }) as DOMRect;
  list.getBoundingClientRect = () => rect(0, 0, 600, 500);
  for (const el of list.querySelectorAll<HTMLElement>('[data-tag-group]')) {
    el.getBoundingClientRect = () =>
      rect(16, 16 + [...list.querySelectorAll('[data-tag-group]')].indexOf(el) * 144, 560, 120);
  }
  return list;
}

/** A point over the upper or lower half of a group's section (laid out by `layOutGroups`). */
function pointerOver(groupId: string, half: 'upper' | 'lower'): Point {
  const rect = section(groupId).getBoundingClientRect();
  return { clientX: 300, clientY: half === 'upper' ? rect.top + 30 : rect.bottom - 30 };
}

/** In the list, over no section: between the first section (to 136) and the second (from 160). */
const IN_FIRST_GAP: Point = { clientX: 300, clientY: 148 };

/** In the list, below every section a test lays out (three end at 424). */
const BELOW_GROUPS: Point = { clientX: 300, clientY: 460 };

/** Past the list's left edge, over the Settings sidebar. */
const LEFT_OF_LIST: Point = { clientX: -20, clientY: 200 };

/**
 * A mouse's pointer event at `at`, as the engines send it: the primary
 * button, down for all but the release.
 */
function pointerEvent(
  type: string,
  target: EventTarget,
  at: Point,
  init: PointerEventInit = {}
): PointerEvent {
  const event = new PointerEvent(type, {
    bubbles: true,
    cancelable: true,
    pointerId: 1,
    pointerType: 'mouse',
    isPrimary: true,
    button: 0,
    buttons: type === 'pointerup' ? 0 : 1,
    ...at,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

/** Where a press on a group's handle lands: at the left of its section's header. */
function pressPoint(groupId: string): Point {
  const rect = section(groupId).getBoundingClientRect();
  return { clientX: rect.left + 8, clientY: rect.top + 12 };
}

/** Press a group's handle. No drag yet: the pointer has to move first. */
function pressGroup(groupId: string, init: PointerEventInit = {}) {
  pointerEvent('pointerdown', groupHandle(groupId), pressPoint(groupId), init);
}

/** Move the pressed pointer to `at`. The drag listens on the window, whatever is under it. */
function movePointer(at: Point) {
  pointerEvent('pointermove', document.body, at);
  flushSync();
}

/** Let the pointer go at `at`. */
async function releasePointer(at: Point) {
  pointerEvent('pointerup', document.body, at);
  await settle();
}

/** Pick a group up by its handle and hold it over a half of group `onto`'s section. */
function dragGroupOver(groupId: string, onto: string, half: 'upper' | 'lower') {
  pressGroup(groupId);
  movePointer(pointerOver(onto, half));
}

/** Let go of the group being dragged over a half of group `onto`'s section. */
async function dropGroupOn(onto: string, half: 'upper' | 'lower') {
  await releasePointer(pointerOver(onto, half));
}

/** Drag a group by its handle and drop it on a half of group `onto`'s section. */
async function dragGroup(groupId: string, onto: string, half: 'upper' | 'lower') {
  dragGroupOver(groupId, onto, half);
  await dropGroupOn(onto, half);
}

/**
 * What the cursor says while a group is dragged, from the mark the drag
 * leaves on the root: `active` where a release would move the group,
 * `refused` where it would not, and null when nothing is being dragged.
 */
const dragMark = () => document.documentElement.getAttribute(POINTER_DRAG_ATTRIBUTE);

/** Each group section showing a drop line, as `groupId:side`. */
function groupDropLines(root: ParentNode): string[] {
  return [...root.querySelectorAll<HTMLElement>('[data-tag-group]')].flatMap((el) =>
    ['before', 'after']
      .filter((side) => el.classList.contains(`drop-${side}`))
      .map((side) => `${el.dataset.tagGroup}:${side}`)
  );
}

/** The group whose section is dimmed as the one being dragged, if any. */
function draggedGroup(root: ParentNode): string | null {
  return root.querySelector<HTMLElement>('.tag-group.dragging')?.dataset.tagGroup ?? null;
}

/** The group ids in position order. */
function groupIds(state: TagState): string[] {
  return [...state.groups].sort((a, b) => a.position - b.position).map((g) => g.id);
}

/** Focus a field and type `text` into it, as keystrokes would (no Enter). */
function type(input: HTMLInputElement, text: string) {
  input.focus();
  input.value = text;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

/**
 * Another writer changed the catalog: the backend's `tags-changed` echo
 * drops the cache and the store refetches.
 */
async function echo(state: TagState) {
  backend.state = state;
  await tagsStore.load({ force: true });
  await settle();
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
  // The writes change what the next get_tag_state returns, as the backend's
  // would; the catalog patches mirror the backend's rules.
  commandsMock.moveTag.mockImplementation(async (tagId, groupId, index) => {
    backend.state = withTagMoved(backend.state, { tagId, groupId, index });
  });
  commandsMock.reorderTagGroups.mockImplementation(async (ids) => {
    backend.state = withGroupOrder(backend.state, ids);
  });
  commandsMock.deleteTagGroup.mockImplementation(async (groupId, mode) => {
    backend.state = withoutGroup(backend.state, groupId, mode);
  });
  commandsMock.deleteTag.mockImplementation(async (tagId: string) => {
    backend.state = withoutTag(backend.state, tagId);
  });
  commandsMock.updateTag.mockImplementation(async (tagId, value) => {
    backend.state = withTagValue(backend.state, tagId, value);
  });
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('TagsSettingsPanel', () => {
  it('renders each group with its tags and usage, then the ungrouped tags', async () => {
    const root = await mountPanel(
      tagState([
        { projectId: 'a', tagId: 'doing' },
        { projectId: 'b', tagId: 'doing' },
        { projectId: 'a', tagId: 'q4' },
      ])
    );

    expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority']);
    expect(nameInputs(root, 'Tag name')).toEqual([
      'Idea',
      'Doing',
      'Done',
      'P0',
      'P1',
      'q4',
      'infra',
    ]);
    expect(row('doing').textContent).toContain('Used by 2');
    expect(row('q4').textContent).toContain('Used by 1');
    expect(row('idea').textContent).toContain('Unused');

    const sections = root.querySelectorAll('section');
    expect(sections[0].textContent).toContain('Project icon');
    expect(sections[1].textContent).not.toContain('Project icon');
    expect(sections[2].textContent).toContain('Ungrouped tags');
  });

  it('creates a group from a template, with its tags and a free name', async () => {
    const root = await mountPanel(tagState());

    const content = await openMenu(root.querySelector<HTMLElement>('.new-tag-group-trigger')!);
    menuItem(content, 'Priority').click();
    await settle();

    // Priority is taken, so the new group is numbered.
    expect(commandsMock.createTagGroup).toHaveBeenCalledWith('Priority 2', [
      { name: 'P0', icon: 'chevrons-up', color: 'red' },
      { name: 'P1', icon: 'chevron-up', color: 'amber' },
      { name: 'P2', icon: 'minus', color: 'blue' },
      { name: 'P3', icon: 'chevron-down', color: 'gray' },
    ]);
    expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Priority 2']);
  });

  describe('pinning a board to the sidebar', () => {
    function groupMenuTrigger(groupName: string): HTMLElement {
      return document.querySelector<HTMLElement>(`[aria-label="Actions for ${groupName}"]`)!;
    }

    it('toggles the group flag from its menu and shows the badge', async () => {
      commandsMock.setTagGroupSidebar.mockImplementation(async (groupId, shown) => {
        backend.state = {
          ...backend.state,
          groups: backend.state.groups.map((g) =>
            g.id === groupId ? { ...g, showInSidebar: shown } : g
          ),
        };
      });
      const root = await mountPanel(tagState());
      expect(root.textContent).not.toContain('In sidebar');

      let item = menuItem(await openMenu(groupMenuTrigger('Priority')), 'Show board in sidebar');
      expect(item.getAttribute('aria-checked')).toBe('false');
      item.click();
      await settle();
      expect(commandsMock.setTagGroupSidebar).toHaveBeenCalledWith('priority', true);
      // The local patch lands at once, and the icon flag is untouched.
      const priority = root.querySelector<HTMLElement>('[aria-labelledby$="priority"]')!;
      expect(priority.textContent).toContain('In sidebar');
      expect(priority.textContent).not.toContain('Project icon');

      item = menuItem(await openMenu(groupMenuTrigger('Priority')), 'Show board in sidebar');
      expect(item.getAttribute('aria-checked')).toBe('true');
      item.click();
      await settle();
      expect(commandsMock.setTagGroupSidebar).toHaveBeenLastCalledWith('priority', false);
      expect(root.textContent).not.toContain('In sidebar');
    });
  });

  describe('deleting a group', () => {
    async function openDeleteDialog(groupName: string) {
      const trigger = document.querySelector<HTMLElement>(
        `[aria-label="Actions for ${groupName}"]`
      )!;
      menuItem(await openMenu(trigger), 'Delete group…').click();
      await settle();
      return dialog();
    }

    it('ungroups its tags by default', async () => {
      await mountPanel(tagState([{ projectId: 'a', tagId: 'p0' }]));
      const body = await openDeleteDialog('Priority');

      expect(body.textContent).toContain('Delete “Priority”?');
      expect(body.textContent).toContain('Keep its 2 tags as ungrouped tags');
      expect(body.textContent).toContain('1 project loses theirs');
      expect(body.querySelector<HTMLInputElement>('input[value="ungroup"]')!.checked).toBe(true);

      button(body, 'Delete group').click();
      await settle();
      expect(commandsMock.deleteTagGroup).toHaveBeenCalledWith('priority', 'ungroup');
    });

    it('deletes the tags too when asked', async () => {
      await mountPanel(tagState());
      const body = await openDeleteDialog('Status');

      expect(body.textContent).toContain('Projects will show their PR status icon instead.');
      const deleteTags = body.querySelector<HTMLInputElement>('input[value="deleteTags"]')!;
      deleteTags.click();
      flushSync();
      button(body, 'Delete group').click();
      await settle();

      expect(commandsMock.deleteTagGroup).toHaveBeenCalledWith('status', 'deleteTags');
      // The group is gone, its tags with it.
      expect(nameInputs(document, 'Group name')).toEqual(['Priority']);
      expect(document.querySelector('[data-tag-row="idea"]')).toBeNull();
    });
  });

  describe('editing a tag', () => {
    it('keeps a rename when a colour is picked before the rename has saved', async () => {
      let finishRename!: () => void;
      commandsMock.updateTag.mockImplementationOnce(
        (tagId, value) =>
          new Promise<void>((resolve) => {
            finishRename = () => {
              backend.state = withTagValue(backend.state, tagId, value);
              resolve();
            };
          })
      );
      await mountPanel(tagState());
      const input = row('idea').querySelector<HTMLInputElement>('input[aria-label="Tag name"]')!;

      // Rename, then tap a swatch before the rename's write has come back:
      // on touch, blur and click fire back to back.
      type(input, 'Spark');
      input.dispatchEvent(new FocusEvent('blur'));
      row('idea').querySelector<HTMLButtonElement>('button[aria-label="red"]')!.click();
      await settle();

      // The colour pick waits for the rename rather than racing it.
      expect(commandsMock.updateTag.mock.calls).toEqual([
        ['idea', { name: 'Spark', color: 'gray', icon: null }],
      ]);

      finishRename();
      await settle();

      // Then it is built on the rename's result, so neither edit is lost.
      expect(commandsMock.updateTag.mock.calls).toEqual([
        ['idea', { name: 'Spark', color: 'gray', icon: null }],
        ['idea', { name: 'Spark', color: 'red', icon: null }],
      ]);
      expect(input.value).toBe('Spark');
      expect(tagsStore.tagsById.get('idea')).toMatchObject({ name: 'Spark', color: 'red' });
    });

    it('keeps a focused tag name draft through a catalog echo, and saves it on blur', async () => {
      await mountPanel(tagState());
      const input = row('idea').querySelector<HTMLInputElement>('input[aria-label="Tag name"]')!;
      type(input, 'Idea-draft');

      // Another window recolours a different tag.
      await echo(withTagValue(backend.state, 'done', { name: 'Done', color: 'red', icon: null }));

      expect(document.activeElement).toBe(input);
      expect(input.value).toBe('Idea-draft');
      // The echo itself landed.
      expect(tagsStore.tagsById.get('done')?.color).toBe('red');

      input.dispatchEvent(new FocusEvent('blur'));
      await settle();
      expect(commandsMock.updateTag).toHaveBeenCalledWith('idea', {
        name: 'Idea-draft',
        color: 'gray',
        icon: null,
      });
    });

    it('follows a rename echo in a tag name field that isn’t focused', async () => {
      await mountPanel(tagState());
      const input = row('idea').querySelector<HTMLInputElement>('input[aria-label="Tag name"]')!;

      await echo(withTagValue(backend.state, 'idea', { name: 'Spark', color: 'gray', icon: null }));

      expect(input.value).toBe('Spark');
    });

    it('keeps a focused group name draft through a catalog echo', async () => {
      const root = await mountPanel(tagState());
      const input = root.querySelector<HTMLInputElement>('input[aria-label="Group name"]')!;
      expect(input.value).toBe('Status');
      type(input, 'Stage');

      await echo(withTagValue(backend.state, 'done', { name: 'Done', color: 'red', icon: null }));

      expect(document.activeElement).toBe(input);
      expect(input.value).toBe('Stage');

      input.dispatchEvent(new FocusEvent('blur'));
      await settle();
      expect(commandsMock.renameTagGroup).toHaveBeenCalledWith('status', 'Stage');
    });
  });

  describe('moving a tag', () => {
    it('queues quick Move down presses, each from where the last left the tag', async () => {
      const finishes: (() => void)[] = [];
      commandsMock.moveTag.mockImplementation(
        (tagId, groupId, index) =>
          new Promise<void>((resolve) => {
            finishes.push(() => {
              backend.state = withTagMoved(backend.state, { tagId, groupId, index });
              resolve();
            });
          })
      );
      const root = await mountPanel(tagState());

      for (let i = 0; i < 2; i++) {
        const content = await openMenu(
          row('idea').querySelector<HTMLElement>('.tag-row-menu-trigger')!
        );
        menuItem(content, 'Move down').click();
        await settle();
      }

      // Both presses show at once, the second stepping from where the first
      // put the tag, and the backend sees them one at a time, in order.
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);
      expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);

      finishes[0]();
      await settle();
      expect(commandsMock.moveTag.mock.calls).toEqual([
        ['idea', 'status', 1],
        ['idea', 'status', 2],
      ]);
      // The first write landing doesn't throw the second's move away.
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);

      finishes[1]();
      await settle();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);
      expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['doing', 'done', 'idea']);

      // Nowhere further down to go.
      const content = await openMenu(
        row('idea').querySelector<HTMLElement>('.tag-row-menu-trigger')!
      );
      expect(menuItem(content, 'Move down').hasAttribute('data-disabled')).toBe(true);
    });

    it('lets a landed move go from the overlay, so the catalog can diverge after it', async () => {
      const root = await mountPanel(tagState());
      const content = await openMenu(
        row('idea').querySelector<HTMLElement>('.tag-row-menu-trigger')!
      );
      menuItem(content, 'Move down').click();
      await settle();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Idea', 'Done']);
      expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['doing', 'idea', 'done']);

      // Another window puts Idea back on top. With the move still overlaid
      // it would be re-applied over the echo and Idea would stay second.
      await echo(withTagMoved(backend.state, { tagId: 'idea', groupId: 'status', index: 0 }));
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Idea', 'Doing', 'Done']);
    });

    it('skips the moves queued behind one that fails, and sends later ones afresh', async () => {
      const writes: { land: () => void; fail: (e: Error) => void }[] = [];
      commandsMock.moveTag.mockImplementation(
        (tagId, groupId, index) =>
          new Promise<void>((resolve, reject) => {
            writes.push({
              land: () => {
                backend.state = withTagMoved(backend.state, { tagId, groupId, index });
                resolve();
              },
              fail: reject,
            });
          })
      );
      const root = await mountPanel(tagState());
      const moveDown = async () => {
        const content = await openMenu(
          row('idea').querySelector<HTMLElement>('.tag-row-menu-trigger')!
        );
        menuItem(content, 'Move down').click();
        await settle();
      };

      await moveDown();
      await moveDown();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);
      expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);

      writes[0].fail(new Error('db locked'));
      await settle();
      // Server truth is back, the second move (resolved against an order
      // that never happened) was never sent, and the one failure toasted.
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Idea', 'Doing', 'Done']);
      expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'status', 1]]);
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(toastError).toHaveBeenCalledWith('Unable to move tag', { description: 'db locked' });

      // A move made after the failure starts over from the server's order.
      await moveDown();
      expect(commandsMock.moveTag.mock.calls).toEqual([
        ['idea', 'status', 1],
        ['idea', 'status', 1],
      ]);
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Idea', 'Done']);
      writes[1].land();
      await settle();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Idea', 'Done']);
      expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['doing', 'idea', 'done']);
    });

    it('reorders within a group by drag, showing the new order at once', async () => {
      let finish!: () => void;
      commandsMock.moveTag.mockImplementationOnce(
        (tagId, groupId, index) =>
          new Promise<void>((resolve) => {
            finish = () => {
              backend.state = withTagMoved(backend.state, { tagId, groupId, index });
              resolve();
            };
          })
      );
      const root = await mountPanel(tagState());

      // Drop Idea below Done (the lower half of its row).
      drag('dragstart', row('idea').querySelector('.drag-handle')!);
      drag('dragover', row('done'));
      drag('drop', row('done'));
      await settle();

      expect(commandsMock.moveTag).toHaveBeenCalledWith('idea', 'status', 2);
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);

      finish();
      await settle();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);
    });

    it('carries a dragged tag under its own type, with no text for a name field', async () => {
      const root = await mountPanel(tagState());

      const start = drag('dragstart', row('idea').querySelector('.drag-handle')!, 10, {});
      expect(carried(start).types).toEqual([TAG_DRAG_TYPE]);
      expect(carried(start).getData(TAG_DRAG_TYPE)).toBe('idea');
      expect(carried(start).effectAllowed).toBe('move');

      // A group's name field is in its header, which no row or list answers
      // for. The panel takes nothing there, so the field's own handling
      // stands, and the drag has no text for it to insert.
      const field = section('priority').querySelector<HTMLInputElement>(
        'input[aria-label="Group name"]'
      )!;
      const over = drag('dragover', field, 10, { [TAG_DRAG_TYPE]: 'idea' });
      expect(over.defaultPrevented).toBe(false);
      expect(carried(over).types).not.toContain('text/plain');
      expect(drag('drop', field, 10, { [TAG_DRAG_TYPE]: 'idea' }).defaultPrevented).toBe(false);
      await settle();

      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority']);
      expect(commandsMock.renameTagGroup).not.toHaveBeenCalled();
      expect(commandsMock.moveTag).not.toHaveBeenCalled();
    });

    it('moves a tag that carries its type, within its group and to another', async () => {
      const root = await mountPanel(tagState());
      const data = { [TAG_DRAG_TYPE]: 'idea' };
      const handle = () => row('idea').querySelector<HTMLElement>('.drag-handle')!;

      // Below Done, in its own group.
      drag('dragstart', handle(), 10, {});
      const over = drag('dragover', row('done'), 10, data);
      expect(over.defaultPrevented).toBe(true);
      expect(carried(over).dropEffect).toBe('move');
      flushSync();
      expect(row('done').classList.contains('drop-after')).toBe(true);
      expect(drag('drop', row('done'), 10, data).defaultPrevented).toBe(true);
      drag('dragend', handle(), 10, {});
      await settle();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Doing', 'Done', 'Idea']);

      // Above P1, a row of another group.
      drag('dragstart', handle(), 10, {});
      drag('dragover', row('p1'), -1, data);
      flushSync();
      expect(row('p1').classList.contains('drop-before')).toBe(true);
      drag('drop', row('p1'), -1, data);
      drag('dragend', handle(), 10, {});
      await settle();
      expect(scopeIds(tagsStore.snapshot, 'priority')).toEqual(['p0', 'idea', 'p1']);

      // Onto the ungrouped list, past its rows: out of every group.
      const ungrouped = root.querySelector<HTMLElement>('.ungrouped .tag-list')!;
      drag('dragstart', handle(), 10, {});
      expect(drag('dragover', ungrouped, 10, data).defaultPrevented).toBe(true);
      drag('drop', ungrouped, 10, data);
      await settle();

      expect(commandsMock.moveTag.mock.calls).toEqual([
        ['idea', 'status', 2],
        ['idea', 'priority', 1],
        ['idea', null, 2],
      ]);
      expect(scopeIds(tagsStore.snapshot, null)).toEqual(['q4', 'infra', 'idea']);
      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
    });

    it('leaves text and files alone, even after a tag drag that never ended', async () => {
      const root = await mountPanel(tagState());

      // A tag drag that got as far as a place it would move to, and whose
      // source then left the page: no drop, and no dragend.
      drag('dragstart', row('idea').querySelector('.drag-handle')!, 10, {});
      drag('dragover', row('done'), 10, { [TAG_DRAG_TYPE]: 'idea' });
      flushSync();
      expect(row('done').classList.contains('drop-after')).toBe(true);

      // Text and files dragged in after it are not that tag, on a row or on
      // the rest of a list.
      const list = section('priority').querySelector<HTMLElement>('.tag-list')!;
      const foreign: Record<string, string>[] = [{ 'text/plain': 'Idea' }, { Files: '' }];
      for (const data of foreign) {
        for (const el of [row('p0'), list]) {
          expect(drag('dragover', el, 10, data).defaultPrevented).toBe(false);
          expect(drag('drop', el, 10, data).defaultPrevented).toBe(false);
        }
      }
      await settle();

      expect(commandsMock.moveTag).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Tag name').slice(0, 3)).toEqual(['Idea', 'Doing', 'Done']);
      // The line stayed where the tag drag left it.
      expect(row('done').classList.contains('drop-after')).toBe(true);
    });

    it('asks first when projects already have a tag in the target group', async () => {
      await mountPanel(
        tagState([
          { projectId: 'a', tagId: 'q4' },
          { projectId: 'a', tagId: 'doing' },
          { projectId: 'b', tagId: 'q4' },
        ])
      );

      drag('dragstart', row('q4').querySelector('.drag-handle')!);
      drag('dragover', row('done'));
      drag('drop', row('done'));
      await settle();

      expect(commandsMock.moveTag).not.toHaveBeenCalled();
      const body = dialog();
      expect(body.textContent).toContain('Move ‘q4’ to Status?');
      expect(body.textContent).toContain('1 project already has a Status.');

      button(body, 'Move').click();
      await settle();
      expect(commandsMock.moveTag).toHaveBeenCalledWith('q4', 'status', 3);
      expect(scopeIds(tagsStore.snapshot, 'status')).toEqual(['idea', 'doing', 'done', 'q4']);
    });

    it('moves without asking when no project conflicts, from the row menu', async () => {
      await mountPanel(tagState([{ projectId: 'a', tagId: 'q4' }]));

      const content = await openMenu(
        row('q4').querySelector<HTMLElement>('.tag-row-menu-trigger')!
      );
      menuItem(content, 'Move to group').click();
      await settle();
      menuItem(document, 'Priority').click();
      await settle();

      expect(commandsMock.moveTag).toHaveBeenCalledWith('q4', 'priority', 2);
      expect(dialog()).toBeNull();
    });

    it('refuses a name the target group already has', async () => {
      const state = tagState();
      state.tags.find((t) => t.id === 'q4')!.name = 'done';
      await mountPanel(state);

      drag('dragstart', row('q4').querySelector('.drag-handle')!);
      drag('dragover', row('done'));
      drag('drop', row('done'));
      await settle();

      expect(commandsMock.moveTag).not.toHaveBeenCalled();
      expect(toastError).toHaveBeenCalledWith('Status already has a tag named “Done”', {
        description: 'Rename one of them first.',
      });
    });
  });

  describe('reordering groups', () => {
    /** `tagState()` with a third group, Area, after Priority. */
    function threeGroups(): TagState {
      const state = tagState();
      state.groups.push(group('area', 2, { name: 'Area' }));
      return state;
    }

    /** Hold every reorder_tag_groups open; each entry settles one, in order. */
    function holdReorders(): { land: () => void; fail: (e: Error) => void }[] {
      const writes: { land: () => void; fail: (e: Error) => void }[] = [];
      commandsMock.reorderTagGroups.mockImplementation(
        (ids) =>
          new Promise<void>((resolve, reject) => {
            writes.push({
              land: () => {
                backend.state = withGroupOrder(backend.state, ids);
                resolve();
              },
              fail: reject,
            });
          })
      );
      return writes;
    }

    async function moveDown(groupName: string) {
      const trigger = document.querySelector<HTMLElement>(
        `[aria-label="Actions for ${groupName}"]`
      )!;
      menuItem(await openMenu(trigger), 'Move down').click();
      await settle();
    }

    it('reorders by drag from the group handle, showing the new order at once', async () => {
      const writes = holdReorders();
      const root = await mountPanel(tagState());
      layOutGroups(root);

      // Status over the lower half of Priority: the line goes below Priority.
      dragGroupOver('status', 'priority', 'lower');
      expect(draggedGroup(root)).toBe('status');
      expect(groupDropLines(root)).toEqual(['priority:after']);
      expect(dragMark()).toBe('active');

      await dropGroupOn('priority', 'lower');

      // The write is still open, and the panel already shows the new order.
      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'status']]]);
      expect(groupIds(tagsStore.snapshot)).toEqual(['status', 'priority']);
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Status']);
      // Each group's tags went with it.
      expect(nameInputs(root, 'Tag name').slice(0, 5)).toEqual([
        'P0',
        'P1',
        'Idea',
        'Doing',
        'Done',
      ]);
      // The release ended the drag.
      expect(draggedGroup(root)).toBeNull();
      expect(groupDropLines(root)).toEqual([]);
      expect(dragMark()).toBeNull();

      writes[0].land();
      await settle();
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Status']);
      expect(groupIds(tagsStore.snapshot)).toEqual(['priority', 'status']);
      expect(commandsMock.moveTag).not.toHaveBeenCalled();
    });

    it('lands a group above the one whose upper half it is dropped on', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      dragGroupOver('area', 'status', 'upper');
      expect(groupDropLines(root)).toEqual(['status:before']);
      await dropGroupOn('status', 'upper');

      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['area', 'status', 'priority']]]);
      expect(nameInputs(root, 'Group name')).toEqual(['Area', 'Status', 'Priority']);
      // The pointer crossed Status's rows on the way, and no tag moved.
      expect(commandsMock.moveTag).not.toHaveBeenCalled();
    });

    it('does not drag for a press that does not move, or moves under the threshold', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);
      const pressed = pressPoint('status');

      pressGroup('status');
      flushSync();
      expect(draggedGroup(root)).toBeNull();
      await releasePointer(pressed);

      // 3px is short of the 4px a drag takes.
      pressGroup('status');
      movePointer({ clientX: pressed.clientX, clientY: pressed.clientY + 3 });
      expect(draggedGroup(root)).toBeNull();
      expect(dragMark()).toBeNull();
      await releasePointer(pointerOver('area', 'lower'));

      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('takes a release in the gap between sections, where the line is drawn', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      // Status held below Priority, then in the gap under it, where that
      // line is drawn: over neither section.
      const gapBelowPriority = { clientX: 300, clientY: 292 };
      dragGroupOver('status', 'priority', 'lower');
      movePointer(gapBelowPriority);
      expect(groupDropLines(root)).toEqual(['priority:after']);
      expect(dragMark()).toBe('active');

      await releasePointer(gapBelowPriority);

      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'status', 'area']]]);
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Status', 'Area']);
    });

    it('takes a release below the last group, where the last section put the line', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      dragGroupOver('status', 'area', 'lower');
      movePointer(BELOW_GROUPS);
      expect(groupDropLines(root)).toEqual(['area:after']);

      await releasePointer(BELOW_GROUPS);

      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'area', 'status']]]);
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Area', 'Status']);
    });

    it('has nowhere to land in a gap before a section has named a place', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      // Straight from the handle into the gap under Status.
      pressGroup('status');
      movePointer(IN_FIRST_GAP);
      expect(draggedGroup(root)).toBe('status');
      expect(groupDropLines(root)).toEqual([]);
      expect(dragMark()).toBe('refused');

      await releasePointer(IN_FIRST_GAP);

      expect(draggedGroup(root)).toBeNull();
      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('forgets the line once the pointer leaves the list, until a section is crossed again', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      dragGroupOver('status', 'priority', 'lower');
      expect(groupDropLines(root)).toEqual(['priority:after']);

      // Off the list, over the Settings sidebar: a release there does
      // nothing, so no line says otherwise.
      movePointer(LEFT_OF_LIST);
      expect(groupDropLines(root)).toEqual([]);
      expect(dragMark()).toBe('refused');
      // The group is still being dragged.
      expect(draggedGroup(root)).toBe('status');

      // Back in through the gap: nowhere to land until a section says where.
      movePointer(BELOW_GROUPS);
      expect(groupDropLines(root)).toEqual([]);
      expect(dragMark()).toBe('refused');

      movePointer(pointerOver('area', 'lower'));
      expect(groupDropLines(root)).toEqual(['area:after']);
      expect(dragMark()).toBe('active');
      await releasePointer(BELOW_GROUPS);

      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'area', 'status']]]);
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Area', 'Status']);
    });

    it('does nothing for a release outside the list', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      // The last move was over a place that moves the group. The release is
      // resolved at its own point.
      dragGroupOver('status', 'priority', 'lower');
      await releasePointer(LEFT_OF_LIST);

      expect(draggedGroup(root)).toBeNull();
      expect(groupDropLines(root)).toEqual([]);
      expect(dragMark()).toBeNull();
      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('lands the group where it is released, over another section than the last move', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      dragGroupOver('status', 'priority', 'lower');
      await dropGroupOn('area', 'lower');

      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'area', 'status']]]);
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Area', 'Status']);
    });

    it('ignores a drop that would leave the groups in their order', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      // Above Priority is where Status already is: no line, and no write.
      dragGroupOver('status', 'priority', 'upper');
      expect(groupDropLines(root)).toEqual([]);
      // Nor is there a line in the gap, where a release lands on the same place.
      movePointer(IN_FIRST_GAP);
      expect(groupDropLines(root)).toEqual([]);
      await dropGroupOn('priority', 'upper');

      // Nor does a group dropped on itself move.
      dragGroupOver('status', 'status', 'lower');
      expect(groupDropLines(root)).toEqual([]);
      await dropGroupOn('status', 'lower');

      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('says through the cursor when a release would not move the group', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      // Below Priority is a move.
      dragGroupOver('status', 'priority', 'lower');
      expect(dragMark()).toBe('active');

      // Above Priority is where Status already is, and so is the gap above
      // that, which keeps the place.
      movePointer(pointerOver('priority', 'upper'));
      expect(dragMark()).toBe('refused');
      movePointer(IN_FIRST_GAP);
      expect(dragMark()).toBe('refused');

      // So is the group's own section, either half.
      for (const half of ['upper', 'lower'] as const) {
        movePointer(pointerOver('status', half));
        expect(dragMark()).toBe('refused');
      }

      // Back over a half that moves, the cursor says so again.
      movePointer(pointerOver('area', 'lower'));
      expect(dragMark()).toBe('active');
      expect(groupDropLines(root)).toEqual(['area:after']);

      await dropGroupOn('status', 'lower');
      expect(dragMark()).toBeNull();
      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('writes nothing when Escape ends the drag', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      dragGroupOver('status', 'area', 'lower');
      expect(groupDropLines(root)).toEqual(['area:after']);

      const escape = new KeyboardEvent('keydown', {
        key: 'Escape',
        bubbles: true,
        cancelable: true,
      });
      document.body.dispatchEvent(escape);
      flushSync();

      // The drag took the key, so Settings stays open, and is over with the
      // button still down.
      expect(escape.defaultPrevented).toBe(true);
      expect(dragMark()).toBeNull();
      expect(groupDropLines(root)).toEqual([]);
      expect(draggedGroup(root)).toBeNull();

      // Neither a move nor the release after it brings it back.
      movePointer(pointerOver('area', 'lower'));
      expect(groupDropLines(root)).toEqual([]);
      await dropGroupOn('area', 'lower');

      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('swallows the click the release sends after a drag, and not the next one', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);
      const clicks = vi.fn();
      root.addEventListener('click', clicks);

      dragGroupOver('status', 'priority', 'lower');
      // An engine sends the click in the same task as the release.
      pointerEvent('pointerup', document.body, pointerOver('priority', 'lower'));
      groupHandle('status').click();
      expect(clicks).not.toHaveBeenCalled();
      await settle();

      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'status', 'area']]]);

      // A click made afterwards is its own.
      groupHandle('status').click();
      expect(clicks).toHaveBeenCalledTimes(1);
    });

    it('drags by the handle only, not by the name field or the menu trigger', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);
      const head = section('status').querySelector<HTMLElement>('.group-head')!;
      const others = [
        head.querySelector<HTMLElement>('input[aria-label="Group name"]')!,
        head.querySelector<HTMLElement>('[aria-label="Actions for Status"]')!,
      ];

      for (const el of others) {
        pointerEvent('pointerdown', el, pressPoint('status'));
        movePointer(pointerOver('area', 'lower'));
        expect(draggedGroup(root)).toBeNull();
        expect(dragMark()).toBeNull();
        await releasePointer(pointerOver('area', 'lower'));
      }

      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('does not drag a group under a finger', async () => {
      const root = await mountPanel(threeGroups());
      layOutGroups(root);

      pressGroup('status', { pointerType: 'touch' });
      pointerEvent('pointermove', document.body, pointerOver('area', 'lower'), {
        pointerType: 'touch',
      });
      flushSync();
      expect(draggedGroup(root)).toBeNull();
      expect(dragMark()).toBeNull();
      pointerEvent('pointerup', document.body, pointerOver('area', 'lower'), {
        pointerType: 'touch',
      });
      await settle();

      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
    });

    it('refuses a native drag from a group handle, and leaves a tag row’s alone', async () => {
      await mountPanel(tagState());

      // A group's drag is made of pointer events. A native one starting in
      // its handle would take the pointer away.
      expect(drag('dragstart', groupHandle('status'), 10, {}).defaultPrevented).toBe(true);

      // A tag's drag is still an HTML5 one.
      const start = drag('dragstart', row('idea').querySelector('.drag-handle')!, 10, {});
      expect(start.defaultPrevented).toBe(false);
      expect(carried(start).types).toEqual([TAG_DRAG_TYPE]);
    });

    it('leaves a tag drag to the rows and lists', async () => {
      const root = await mountPanel(tagState());

      // A tag over a group's header is not a group drop.
      drag('dragstart', row('idea').querySelector('.drag-handle')!);
      const head = section('priority').querySelector<HTMLElement>('.group-head')!;
      expect(drag('dragover', head).defaultPrevented).toBe(false);
      expect(drag('drop', head).defaultPrevented).toBe(false);
      flushSync();
      expect(groupDropLines(root)).toEqual([]);

      // Dropped on the group's list, the tag joins the end of that group.
      const list = section('priority').querySelector<HTMLElement>('.tag-list')!;
      drag('dragover', list);
      drag('drop', list);
      await settle();

      expect(commandsMock.moveTag.mock.calls).toEqual([['idea', 'priority', 2]]);
      expect(commandsMock.reorderTagGroups).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority']);
      expect(scopeIds(tagsStore.snapshot, 'priority')).toEqual(['p0', 'p1', 'idea']);
    });

    it('moves a group, not a tag, after a tag drag that never ended', async () => {
      const root = await mountPanel(tagState());
      layOutGroups(root);

      // The tag's dragend never comes (its row left the page, say).
      drag('dragstart', row('idea').querySelector('.drag-handle')!);
      drag('dragover', row('done'));
      flushSync();
      expect(row('idea').classList.contains('dragging')).toBe(true);
      expect(row('done').classList.contains('drop-after')).toBe(true);

      dragGroupOver('status', 'priority', 'lower');

      expect(row('idea').classList.contains('dragging')).toBe(false);
      expect(row('done').classList.contains('drop-after')).toBe(false);
      expect(draggedGroup(root)).toBe('status');
      expect(groupDropLines(root)).toEqual(['priority:after']);

      await dropGroupOn('priority', 'lower');

      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'status']]]);
      expect(commandsMock.moveTag).not.toHaveBeenCalled();
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Status']);
    });

    it('scrolls the list while a group is held near its bottom, and the line follows', async () => {
      const root = await mountPanel(threeGroups());
      const list = root.querySelector<HTMLElement>('.panel-body')!;
      // A 300px list of 120px sections 24px apart from 16, which scrolls:
      // Priority, the second, spans 160 to 280, and Area starts at 304,
      // past the list's bottom edge.
      let scrollTop = 0;
      Object.defineProperty(list, 'scrollTop', {
        configurable: true,
        get: () => scrollTop,
        set: (value: number) => (scrollTop = value),
      });
      const rect = (left: number, top: number, width: number, height: number) =>
        ({ left, right: left + width, top, bottom: top + height, width, height }) as DOMRect;
      list.getBoundingClientRect = () => rect(0, 0, 600, 300);
      for (const el of list.querySelectorAll<HTMLElement>('[data-tag-group]')) {
        el.getBoundingClientRect = () =>
          rect(
            16,
            16 + [...list.querySelectorAll('[data-tag-group]')].indexOf(el) * 144 - scrollTop,
            560,
            120
          );
      }
      // Hold the frames, as the drag's scroll loop asks for them.
      const frames = new Map<number, FrameRequestCallback>();
      let frameId = 0;
      vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
        frames.set(++frameId, callback);
        return frameId;
      });
      vi.stubGlobal('cancelAnimationFrame', (id: number) => void frames.delete(id));
      const runFrame = (time: number) => {
        const due = [...frames.values()];
        frames.clear();
        for (const callback of due) callback(time);
        flushSync();
      };
      try {
        // Status, held 20px short of the list's bottom edge, inside its 48px
        // scroll zone, having crossed Priority's lower half on the way.
        const held = { clientX: 300, clientY: 280 };
        pressGroup('status');
        movePointer({ clientX: 300, clientY: 270 });
        movePointer(held);
        expect(groupDropLines(root)).toEqual(['priority:after']);

        // The first frame only starts the clock.
        runFrame(0);
        expect(scrollTop).toBe(0);
        // 28px into the zone is 525px a second: 52px in 100ms, and half a
        // pixel carried.
        runFrame(100);
        expect(scrollTop).toBe(52);
        // The pointer has not moved. Area, now from 252 to 372, has come
        // under it, and its upper half is the same place as below Priority.
        expect(groupDropLines(root)).toEqual(['area:before']);
        // Another 53px, and the pointer is over Area's lower half.
        runFrame(200);
        expect(scrollTop).toBe(105);
        expect(groupDropLines(root)).toEqual(['area:after']);
        expect(dragMark()).toBe('active');

        await releasePointer(held);
        expect(commandsMock.reorderTagGroups.mock.calls).toEqual([
          [['priority', 'area', 'status']],
        ]);
        expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Area', 'Status']);

        // The release ended the scrolling with the drag.
        runFrame(300);
        expect(scrollTop).toBe(105);
      } finally {
        vi.unstubAllGlobals();
      }
    });

    it('queues quick Move down presses, each from where the last left the group', async () => {
      const writes = holdReorders();
      const root = await mountPanel(threeGroups());

      await moveDown('Status');
      await moveDown('Status');

      // Both presses show at once, and the backend sees them one at a time.
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Area', 'Status']);
      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'status', 'area']]]);

      writes[0].land();
      await settle();
      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([
        [['priority', 'status', 'area']],
        [['priority', 'area', 'status']],
      ]);
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Area', 'Status']);

      writes[1].land();
      await settle();
      expect(nameInputs(root, 'Group name')).toEqual(['Priority', 'Area', 'Status']);
      expect(groupIds(tagsStore.snapshot)).toEqual(['priority', 'area', 'status']);

      // Nowhere further down to go.
      const trigger = document.querySelector<HTMLElement>('[aria-label="Actions for Status"]')!;
      expect(menuItem(await openMenu(trigger), 'Move down').hasAttribute('data-disabled')).toBe(
        true
      );
    });

    it('puts the groups back when a reorder fails, and skips the one queued behind it', async () => {
      const writes = holdReorders();
      const root = await mountPanel(threeGroups());

      layOutGroups(root);
      await dragGroup('status', 'area', 'lower');
      await moveDown('Priority');
      expect(nameInputs(root, 'Group name')).toEqual(['Area', 'Priority', 'Status']);

      writes[0].fail(new Error('db locked'));
      await settle();

      expect(nameInputs(root, 'Group name')).toEqual(['Status', 'Priority', 'Area']);
      expect(commandsMock.reorderTagGroups.mock.calls).toEqual([[['priority', 'area', 'status']]]);
      expect(toastError).toHaveBeenCalledTimes(1);
      expect(toastError).toHaveBeenCalledWith('Unable to reorder groups', {
        description: 'db locked',
      });
    });
  });

  it('confirms deleting a tag with how many projects use it', async () => {
    await mountPanel(
      tagState([
        { projectId: 'a', tagId: 'infra' },
        { projectId: 'b', tagId: 'infra' },
      ])
    );

    row('infra').querySelector<HTMLButtonElement>('[aria-label="Delete infra"]')!.click();
    await settle();
    const body = dialog();
    expect(body.textContent).toContain('Delete “infra”? Used by 2 projects, which lose it.');

    button(body, 'Delete').click();
    await settle();
    expect(commandsMock.deleteTag).toHaveBeenCalledWith('infra');
  });
});
