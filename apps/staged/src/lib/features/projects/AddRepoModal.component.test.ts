import { flushSync, mount, tick, unmount } from 'svelte';
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RecentRepo } from '../../types';

// The Add Repository dialog with its real form, repo fields and dialog
// chrome. The recent-repo chips sit inside the form, ahead of Add. While
// they had no `type` each was a submit button: a click on one picked its
// repo and submitted the form in the same press, and a Return, which
// presses a form's first submit button, did that to the first chip. So one
// Return added a repo nobody picked, with Add itself still disabled.
const { commands } = vi.hoisted(() => ({
  commands: {
    listRecentRepos: vi.fn(),
    listUserRepos: vi.fn(),
    searchGithubRepos: vi.fn(),
    getGithubRepo: vi.fn(),
    checkMonorepoModules: vi.fn(),
    detectDefaultBranch: vi.fn(),
    listRepoDirectories: vi.fn(),
    validateSubpath: vi.fn(),
    getPrForRepo: vi.fn(),
    getParentRepo: vi.fn(),
    listPullRequests: vi.fn(),
    listGitBranches: vi.fn(),
  },
}));

vi.mock('../../api/commands', () => commands);
vi.mock('../../stores/repoBadges.svelte', () => ({
  repoBadgeStore: { lookup: () => undefined, ensureForRepos: vi.fn() },
}));

import AddRepoModal from './AddRepoModal.svelte';

const recents: RecentRepo[] = [
  { id: 'r1', githubRepo: 'acme/widgets', subpath: null, lastUsedAt: 2 },
  { id: 'r2', githubRepo: 'acme/gadgets', subpath: 'packages/app', lastUsedAt: 1 },
];

let component: ReturnType<typeof mount> | null = null;

/** Let the dialog open, bits-ui settle its focus and the recents load. */
async function settle() {
  await new Promise((resolve) => setTimeout(resolve, 30));
  await tick();
  flushSync();
}

async function open() {
  const props = { open: true, onAdded: vi.fn(), onClose: vi.fn() };
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(AddRepoModal, { target, props });
  flushSync();
  await settle();
  await vi.waitFor(() => expect(chips()).toHaveLength(recents.length));
  return props;
}

function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>('[data-slot="dialog-content"]');
  if (!found) throw new Error('no dialog');
  return found;
}

function form(): HTMLFormElement {
  return dialog().querySelector('form')!;
}

function chips(): HTMLButtonElement[] {
  return [...document.querySelectorAll<HTMLButtonElement>('.recent-chip')];
}

function addButton(): HTMLButtonElement {
  const found = [...form().querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Add');
  if (!found) throw new Error('no Add button');
  return found;
}

function pressReturn(el: Element) {
  const event = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
  el.dispatchEvent(event);
  flushSync();
  return event;
}

/**
 * What a browser does for a Return typed in one of the form's fields
 * (HTML's implicit submission), which jsdom leaves out: it presses the
 * form's default button, the first one in tree order that submits, and does
 * nothing while that button is disabled.
 */
function browserReturnInField(target: HTMLFormElement) {
  const defaultButton = [...target.elements].find(
    (el): el is HTMLButtonElement => el instanceof HTMLButtonElement && el.type === 'submit'
  );
  if (defaultButton && !defaultButton.disabled) defaultButton.click();
  flushSync();
  return defaultButton;
}

function expectNothingAdded(props: { onAdded: unknown; onClose: unknown }) {
  expect(props.onAdded).not.toHaveBeenCalled();
  expect(props.onClose).not.toHaveBeenCalled();
  // No repo was picked either: the chips are still offered, Add is still off
  // and nothing was looked up for a repo.
  expect(chips()).toHaveLength(recents.length);
  expect(addButton().disabled).toBe(true);
  expect(commands.checkMonorepoModules).not.toHaveBeenCalled();
}

describe('AddRepoModal', () => {
  beforeAll(() => {
    // jsdom has no Web Animations, which the chips' slide-out starts.
    Element.prototype.animate = () => ({ cancel: () => {} }) as unknown as Animation;
  });

  afterAll(() => {
    delete (Element.prototype as Partial<Element>).animate;
  });

  beforeEach(() => {
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        disconnect() {}
        unobserve() {}
      }
    );
    for (const command of Object.values(commands)) command.mockReset().mockResolvedValue([]);
    commands.listRecentRepos.mockResolvedValue(recents);
    commands.checkMonorepoModules.mockResolvedValue(0);
    commands.detectDefaultBranch.mockResolvedValue('main');
    commands.getPrForRepo.mockResolvedValue(null);
    commands.getParentRepo.mockResolvedValue(null);
    commands.getGithubRepo.mockResolvedValue(null);
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
    vi.unstubAllGlobals();
  });

  describe('with recent-repo chips shown and no repo selected', () => {
    it('has Add, which is disabled, as the only button that submits the form', async () => {
      await open();

      for (const chip of chips()) {
        expect(chip.form).toBe(form());
        expect(chip.getAttribute('type')).toBe('button');
      }
      const submitters = [...form().elements].filter(
        (el) => (el as HTMLButtonElement).type === 'submit'
      );
      expect(submitters).toEqual([addButton()]);
      expect(addButton().disabled).toBe(true);
    });

    it('adds nothing on Return on the dialog itself', async () => {
      const props = await open();
      dialog().focus();
      expect(document.activeElement).toBe(dialog());

      pressReturn(document.activeElement!);
      await settle();

      expectNothingAdded(props);
    });

    it('adds nothing on Return on the title', async () => {
      const props = await open();

      pressReturn(dialog().querySelector('[data-slot="dialog-title"]')!);
      await settle();

      expectNothingAdded(props);
    });

    it('adds nothing on the Return a browser handles in the empty search field', async () => {
      const props = await open();

      const pressed = browserReturnInField(form());
      await settle();

      expect(pressed).toBe(addButton());
      expectNothingAdded(props);
    });
  });

  it('picks the repo on a click on its chip, and waits for Add', async () => {
    const props = await open();

    chips()[0].click();
    flushSync();
    await settle();

    expect(props.onAdded).not.toHaveBeenCalled();
    expect(props.onClose).not.toHaveBeenCalled();
    expect(dialog().textContent).toContain('acme/widgets');
    expect(addButton().disabled).toBe(false);
  });

  it('adds the picked repo on Return on the dialog', async () => {
    const props = await open();
    chips()[1].click();
    flushSync();
    await settle();
    dialog().focus();

    const event = pressReturn(document.activeElement!);

    await vi.waitFor(() => expect(props.onAdded).toHaveBeenCalledTimes(1));
    expect(props.onAdded.mock.calls[0][0]).toMatchObject({
      nameWithOwner: 'acme/gadgets',
      subpath: 'packages/app',
    });
    expect(props.onClose).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });
});
