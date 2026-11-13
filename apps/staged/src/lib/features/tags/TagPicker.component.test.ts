// The tag picker's keys over the real tagsStore and tagActions, with the
// dialog chrome stubbed (the mobile layout, so the list renders at once) and
// the commands mocked.
import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Project, TagState } from '../../types';
import { tagState } from './testStubs/tagFixtures';

const { backend } = vi.hoisted(() => ({ backend: { state: null as unknown as TagState } }));

const commandsMock = vi.hoisted(() => ({
  getTagState: vi.fn(async () => ({ data: structuredClone(backend.state), revalidating: null })),
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
  projectsDataStore: { liveProjects: [{ id: 'p1' }], isProjectDeleting: () => false },
}));
vi.mock('../../shared/viewport.svelte', () => ({ viewport: { isMobile: true } }));
vi.mock('../layout/navigation.svelte', () => ({ openSettings: vi.fn() }));
vi.mock('$lib/components/ui/dialog', () =>
  import('../sessions/testStubs/dialogMock').then((m) => m.dialogModuleMock())
);
vi.mock('svelte-sonner', () => ({ toast: { error: vi.fn() } }));

import { tagsStore } from '../../stores/tags.svelte';
import TagPicker from './TagPicker.svelte';

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

/** Open the picker and type `query` into its search box. */
async function search(query: string): Promise<HTMLInputElement> {
  backend.state = tagState();
  await tagsStore.load({ force: true });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(TagPicker, { target, props: { project, open: true } });
  await settle();

  const input = target.querySelector<HTMLInputElement>('input[role="combobox"]')!;
  input.value = query;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  flushSync();
  return input;
}

function pressReturn(el: Element, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', {
    key: 'Enter',
    bubbles: true,
    cancelable: true,
    ...init,
  });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

beforeEach(() => {
  commandsMock.setProjectTagAssigned.mockClear();
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
});

describe('TagPicker Return key', () => {
  it('toggles the highlighted tag on Return', async () => {
    const input = await search('q4');

    const event = pressReturn(input);
    await settle();

    expect(commandsMock.setProjectTagAssigned).toHaveBeenCalledExactlyOnceWith('p1', 'q4', true);
    expect(event.defaultPrevented).toBe(true);
  });

  it('leaves a Return that commits an IME composition to the IME', async () => {
    const input = await search('q4');

    // Mid-composition, then Safari's committing Return: compositionend has
    // already fired, so isComposing is false and only keyCode 229 tells.
    const composing = pressReturn(input, { isComposing: true });
    const committing = pressReturn(input, { keyCode: 229 });
    await settle();

    expect(commandsMock.setProjectTagAssigned).not.toHaveBeenCalled();
    expect(composing.defaultPrevented).toBe(false);
    expect(committing.defaultPrevented).toBe(false);
  });
});
