import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import NoteSelectionReply from './NoteSelectionReply.svelte';
import type { NewDraftQuote } from '../sessions/composerDraft';
import type { LiveQuoteReply } from '../sessions/replyDrafts';
import { viewport } from '../../shared/viewport.svelte';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

// The selection flow with nothing to confirm: the Reply button appears once
// a selection in the note has settled; with the chat composer beside the
// note a press adds the block there (`onAdd`), otherwise it opens a reply
// field whose block is already in the draft (`onReply`) and whose typing is
// handed on as it goes, so dismissing the field just closes it. By touch the
// tap that presses the button collapses the page selection first, and the
// captured quote has to survive that. On a phone the button is docked above
// the bottom bar instead of beside the selection, and the field is a sheet.

const SETTLE_MS = 150;
const TOUCH_SETTLE_MS = 350;
const WHEEL_GRACE_MS = 300;
const DOCK_LEAVE_MS = 150;

let component: ReturnType<typeof mount> | null = null;
let host: HTMLElement;
let scrollEl: HTMLElement;
let contentEl: HTMLElement;

type Live = { [K in keyof LiveQuoteReply]: Mock<LiveQuoteReply[K]> };

function liveReply(): Live {
  return {
    setReply: vi.fn<LiveQuoteReply['setReply']>(),
    finish: vi.fn<LiveQuoteReply['finish']>(async () => {}),
  };
}

function render(
  overrides: Partial<{
    chatOpen: boolean;
    chatVisible: boolean;
    sheetHost: HTMLElement | null;
    dockHost: HTMLElement | null;
    onDockedChange: (docked: boolean) => void;
  }> = {}
) {
  host = document.createElement('div');
  scrollEl = document.createElement('div');
  scrollEl.style.position = 'relative';
  contentEl = document.createElement('div');
  contentEl.innerHTML = `
    <h1 id="title">Rollout plan</h1>
    <h2 id="arch">Architecture</h2>
    <p id="flow-p">The cache is invalidated on every write.</p>
  `;
  scrollEl.appendChild(contentEl);
  host.appendChild(scrollEl);
  document.body.appendChild(host);

  const live = liveReply();
  const onAdd = vi.fn<(quote: NewDraftQuote) => Promise<boolean>>(async () => true);
  const onReply = vi.fn(() => live);
  const props = reactiveProps({
    scrollEl,
    contentEl,
    enabled: true,
    chatOpen: false,
    chatVisible: false,
    sheetHost: null as HTMLElement | null,
    dockHost: null as HTMLElement | null,
    onDockedChange: undefined as ((docked: boolean) => void) | undefined,
    ignoreHeading: (heading: Element) => heading.tagName === 'H1',
    onAdd,
    onReply,
    ...overrides,
  });
  component = mount(NoteSelectionReply, { target: scrollEl, props });
  flushSync();
  return { props, onAdd, onReply, live };
}

/** Select the text of `#flow-p` and let the page know, as a drag or handles would. */
function selectPassage(from = 0, to?: number) {
  const text = contentEl.querySelector('#flow-p')!.firstChild as Text;
  const range = document.createRange();
  range.setStart(text, from);
  range.setEnd(text, to ?? text.length);
  const selection = document.getSelection()!;
  selection.removeAllRanges();
  selection.addRange(range);
  document.dispatchEvent(new Event('selectionchange'));
}

function collapseSelection() {
  document.getSelection()?.removeAllRanges();
  document.dispatchEvent(new Event('selectionchange'));
}

function settle(ms = SETTLE_MS) {
  vi.advanceTimersByTime(ms);
  flushSync();
}

function button(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('.selection-reply-button');
}

function field(): HTMLTextAreaElement | null {
  return document.querySelector<HTMLTextAreaElement>('.selection-reply-input');
}

function dockButton(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>('.selection-reply-dock-button');
}

/** A phone: menus are sheets, and the dialog hosts the reply sheet and the docked button. */
function phoneHosts() {
  viewport.hasCoarsePointer = true;
  viewport.sheetMenus = true;
  viewport.showShortcutHints = false;
  const sheetHost = document.createElement('div');
  // Focusable, as bits-ui's focus scope makes the dialog's content.
  sheetHost.tabIndex = -1;
  const dockHost = document.createElement('div');
  document.body.append(sheetHost, dockHost);
  return {
    sheetHost,
    dockHost,
    cleanup() {
      sheetHost.remove();
      dockHost.remove();
    },
  };
}

function press(el: HTMLElement, pointerType = 'mouse') {
  el.dispatchEvent(
    new (window.PointerEvent ?? MouseEvent)('pointerdown', { bubbles: true, pointerType } as never)
  );
}

function click(el: HTMLElement) {
  el.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  flushSync();
}

async function flushAsync() {
  // Let the click's awaited `onAdd` settle without advancing fake timers.
  await Promise.resolve();
  await Promise.resolve();
  flushSync();
}

beforeEach(() => {
  vi.useFakeTimers();
  // jsdom lays nothing out: give the selected text one line box, so the
  // button has an end to sit under.
  Range.prototype.getClientRects = () =>
    [
      { left: 20, right: 180, top: 40, bottom: 60, width: 160, height: 20 },
    ] as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () =>
    ({ left: 20, right: 180, top: 40, bottom: 60, width: 160, height: 20 }) as DOMRect;
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe() {}
      disconnect() {}
      unobserve() {}
    }
  );
  viewport.hasCoarsePointer = false;
  viewport.sheetMenus = false;
  viewport.showShortcutHints = true;
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  host.remove();
  document.getSelection()?.removeAllRanges();
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe('NoteSelectionReply button', () => {
  it('appears once a selection in the note has settled and goes when it collapses', () => {
    render();

    selectPassage();
    expect(button()).toBeNull();
    settle();
    expect(button()).not.toBeNull();

    collapseSelection();
    flushSync();
    expect(button()).toBeNull();
  });

  it('waits for the mouse to be released before showing', () => {
    render();

    press(contentEl, 'mouse');
    selectPassage();
    settle();
    expect(button()).toBeNull();

    document.dispatchEvent(new (window.PointerEvent ?? MouseEvent)('pointerup', { bubbles: true }));
    settle(0);
    expect(button()).not.toBeNull();
  });

  it('by touch shows after the handles have been still, with no pointer release', () => {
    viewport.hasCoarsePointer = true;
    render();

    press(contentEl, 'touch');
    selectPassage();
    settle(SETTLE_MS);
    expect(button()).toBeNull();
    settle(TOUCH_SETTLE_MS - SETTLE_MS);
    expect(button()).not.toBeNull();
  });
});

describe('NoteSelectionReply with the chat beside the note', () => {
  it('adds the quote straight to the composer and clears the selection', async () => {
    const { onAdd, onReply } = render({ chatVisible: true });
    selectPassage();
    settle();

    click(button()!);
    await flushAsync();

    expect(onAdd).toHaveBeenCalledWith({
      quote: 'The cache is invalidated on every write.',
      headings: ['Architecture'],
      reply: '',
    });
    expect(onReply).not.toHaveBeenCalled();
    expect(field()).toBeNull();
    expect(button()).toBeNull();
    expect(document.getSelection()?.isCollapsed).toBe(true);
  });

  it('keeps the button for another try when the block could not be added', async () => {
    const { onAdd } = render({ chatVisible: true });
    onAdd.mockResolvedValueOnce(false);
    selectPassage();
    settle();

    click(button()!);
    await flushAsync();

    expect(button()).not.toBeNull();
  });
});

describe('NoteSelectionReply reply field', () => {
  it('opens with the block already in the draft, types into it, and Enter closes it', () => {
    const { onReply, live } = render();
    selectPassage();
    settle();

    click(button()!);

    expect(onReply).toHaveBeenCalledWith({
      quote: 'The cache is invalidated on every write.',
      headings: ['Architecture'],
      reply: '',
    });
    const input = field();
    expect(input).not.toBeNull();
    // Nothing to confirm: no buttons in the field.
    expect(document.querySelectorAll('[data-slot="popover-content"] button')).toHaveLength(0);

    input!.value = 'Why on every write?';
    input!.dispatchEvent(new Event('input', { bubbles: true }));
    expect(live.setReply).toHaveBeenCalledWith('Why on every write?');

    const enter = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
    input!.dispatchEvent(enter);
    flushSync();
    expect(enter.defaultPrevented).toBe(true);
    expect(live.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
    expect(button()).toBeNull();
    expect(document.getSelection()?.isCollapsed).toBe(true);
  });

  it('lets Shift+Enter through for a new line', () => {
    const { live } = render();
    selectPassage();
    settle();
    click(button()!);

    const enter = new KeyboardEvent('keydown', {
      key: 'Enter',
      shiftKey: true,
      bubbles: true,
      cancelable: true,
    });
    field()!.dispatchEvent(enter);
    flushSync();

    expect(enter.defaultPrevented).toBe(false);
    expect(live.finish).not.toHaveBeenCalled();
    expect(field()).not.toBeNull();
  });

  it('closes, keeping the block, when the chat opens beside the note', () => {
    const { props, live } = render();
    selectPassage();
    settle();
    click(button()!);
    expect(field()).not.toBeNull();

    props.chatOpen = true;
    props.chatVisible = true;
    flushSync();

    expect(live.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
  });

  it('keeps Escape from the dialog around it', () => {
    render();
    selectPassage();
    settle();
    click(button()!);
    const reachedDocument = vi.fn();
    document.addEventListener('keydown', reachedDocument);

    try {
      const escape = new KeyboardEvent('keydown', {
        key: 'Escape',
        bubbles: true,
        cancelable: true,
      });
      field()!.dispatchEvent(escape);
      flushSync();

      expect(escape.defaultPrevented).toBe(true);
      expect(reachedDocument).not.toHaveBeenCalled();
      expect(field()).toBeNull();
    } finally {
      document.removeEventListener('keydown', reachedDocument);
    }
  });

  it('closes when the note is scrolled with the wheel, once the trackpad has had time to settle', () => {
    const { live } = render();
    selectPassage();
    settle();
    click(button()!);

    // Inertia from the scroll before the selection: not a scroll away.
    scrollEl.dispatchEvent(new Event('wheel', { bubbles: true }));
    flushSync();
    expect(field()).not.toBeNull();

    settle(WHEEL_GRACE_MS);
    scrollEl.dispatchEvent(new Event('wheel', { bubbles: true }));
    flushSync();

    expect(live.finish).toHaveBeenCalledTimes(1);
    expect(field()).toBeNull();
  });

  it('writes what was typed when the note closes on an open field', () => {
    const { live } = render();
    selectPassage();
    settle();
    click(button()!);

    unmount(component!);
    component = null;

    expect(live.finish).toHaveBeenCalledTimes(1);
  });
});

describe('NoteSelectionReply by touch', () => {
  it('survives the tap collapsing the page selection before the button’s click', async () => {
    viewport.hasCoarsePointer = true;
    const { onAdd } = render({ chatVisible: true });
    selectPassage();
    settle(TOUCH_SETTLE_MS);
    const target = button()!;

    // iOS: the touch lands on the button, the selection goes, then the click.
    press(target, 'touch');
    collapseSelection();
    flushSync();
    expect(button()).toBe(target);

    click(target);
    await flushAsync();

    expect(onAdd).toHaveBeenCalledTimes(1);
    expect(onAdd.mock.calls[0][0]).toMatchObject({
      quote: 'The cache is invalidated on every write.',
    });
  });

  it('lets a press that never clicks drop the button again', () => {
    viewport.hasCoarsePointer = true;
    render();
    selectPassage();
    settle(TOUCH_SETTLE_MS);

    press(button()!, 'touch');
    collapseSelection();
    flushSync();
    expect(button()).not.toBeNull();

    settle(600);
    expect(button()).toBeNull();
  });

  it('opens the reply field as a sheet in the dialog, dismissed by a tap on its scrim', () => {
    const hosts = phoneHosts();
    try {
      const { live } = render(hosts);
      selectPassage();
      settle(TOUCH_SETTLE_MS);

      click(dockButton()!);

      const sheet = hosts.sheetHost.querySelector('.selection-reply-sheet');
      expect(sheet).not.toBeNull();
      expect(document.querySelector('[data-slot="popover-content"]')).toBeNull();
      expect(document.activeElement).toBe(field());
      expect(sheet!.textContent).not.toContain('Esc');

      const scrim = sheet!.querySelector<HTMLElement>('.selection-reply-scrim')!;
      press(scrim, 'touch');
      click(scrim);

      expect(live.finish).toHaveBeenCalledTimes(1);
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).toBeNull();
    } finally {
      hosts.cleanup();
    }
  });

  it('hands focus back to the dialog when Return closes the sheet', () => {
    const hosts = phoneHosts();
    try {
      render(hosts);
      selectPassage();
      settle(TOUCH_SETTLE_MS);
      click(dockButton()!);
      expect(document.activeElement).toBe(field());

      field()!.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })
      );
      flushSync();

      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).toBeNull();
      expect(document.activeElement).toBe(hosts.sheetHost);
    } finally {
      hosts.cleanup();
    }
  });

  it('closes the sheet, keeping the block, when the chat opens over the note', () => {
    const hosts = phoneHosts();
    try {
      const { props, live } = render(hosts);
      selectPassage();
      settle(TOUCH_SETTLE_MS);
      click(dockButton()!);
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).not.toBeNull();

      // Over the note, not beside it: `chatVisible` stays false on a phone.
      props.chatOpen = true;
      flushSync();

      expect(live.finish).toHaveBeenCalledTimes(1);
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).toBeNull();
    } finally {
      hosts.cleanup();
    }
  });
});

describe('NoteSelectionReply docked on a phone', () => {
  it('docks a Reply row with the selected text above the bottom bar instead of a button by the selection', () => {
    const hosts = phoneHosts();
    const onDockedChange = vi.fn();
    try {
      const { onReply } = render({ ...hosts, onDockedChange });
      selectPassage();
      settle(TOUCH_SETTLE_MS);

      expect(button()).toBeNull();
      const docked = dockButton()!;
      expect(docked).not.toBeNull();
      expect(hosts.dockHost.contains(docked)).toBe(true);
      expect(docked.textContent).toContain('Reply to selection');
      expect(docked.textContent).toContain('The cache is invalidated on every write.');
      expect(onDockedChange).toHaveBeenLastCalledWith(true);

      click(docked);

      expect(onReply).toHaveBeenCalledWith({
        quote: 'The cache is invalidated on every write.',
        headings: ['Architecture'],
        reply: '',
      });
      expect(hosts.sheetHost.querySelector('.selection-reply-sheet')).not.toBeNull();
    } finally {
      hosts.cleanup();
    }
  });

  it('withdraws the docked row, after its leaving animation, when the selection collapses', () => {
    const hosts = phoneHosts();
    const onDockedChange = vi.fn();
    try {
      render({ ...hosts, onDockedChange });
      selectPassage();
      settle(TOUCH_SETTLE_MS);
      expect(dockButton()).not.toBeNull();

      collapseSelection();
      flushSync();

      const leaving = hosts.dockHost.querySelector('.selection-reply-dock');
      expect(leaving?.classList.contains('is-leaving')).toBe(true);
      expect(onDockedChange).toHaveBeenLastCalledWith(true);

      settle(DOCK_LEAVE_MS);

      expect(dockButton()).toBeNull();
      expect(onDockedChange).toHaveBeenLastCalledWith(false);
    } finally {
      hosts.cleanup();
    }
  });

  it('keeps the button by the selection on a coarse pointer without sheet menus', () => {
    viewport.hasCoarsePointer = true;
    viewport.sheetMenus = false;
    const dockHost = document.createElement('div');
    document.body.appendChild(dockHost);
    try {
      render({ dockHost });
      selectPassage();
      settle(TOUCH_SETTLE_MS);

      expect(dockButton()).toBeNull();
      expect(button()).not.toBeNull();
    } finally {
      dockHost.remove();
    }
  });
});
