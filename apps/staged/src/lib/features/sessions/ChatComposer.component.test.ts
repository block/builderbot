import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

/**
 * The submit control's dropdown: the Queue / Send mode switch, and, where the
 * host can start a new session from the draft (the note dialog), the
 * "New note" / "New commit" actions.
 */

// The menu's parts render inline: the trigger and content pass their children
// through, and an item is a plain button, so entries can be read and picked
// without opening a floating menu under jsdom.
vi.mock('$lib/components/ui/dropdown-menu', async () => {
  const { default: Passthrough } = await import('./testStubs/PassthroughStub.svelte');
  const { default: MenuItem } = await import('./testStubs/MenuItemStub.svelte');
  const { default: Empty } = await import('./testStubs/EmptyStub.svelte');
  return {
    Root: Passthrough,
    Trigger: Passthrough,
    Content: Passthrough,
    RadioGroup: Passthrough,
    RadioItem: MenuItem,
    Item: MenuItem,
    Separator: Empty,
  };
});
vi.mock('./HashtagInput.svelte', () => import('./testStubs/PromptInputStub.svelte'));
vi.mock('./AttachmentThumb.svelte', () => import('./testStubs/EmptyStub.svelte'));

import ChatComposer from './ChatComposer.svelte';
import { composerSendControl, type ComposerNewSessionKind } from './composerSendControl';

let component: ReturnType<typeof mount> | null = null;

/**
 * Mounts the composer with the control the pane would hand it for this
 * session state (`isLive`, `canSendLive`) and host (`onStartNewSession`).
 */
function mountComposer({
  canSendLive = false,
  ...props
}: {
  isLive?: boolean;
  canSendLive?: boolean;
  onStartNewSession?: (kind: ComposerNewSessionKind) => void;
  [prop: string]: unknown;
}) {
  const sendControl = composerSendControl({
    isLive: props.isLive ?? false,
    canSendLive,
    mode: 'queue',
    canStartNewSession: props.onStartNewSession !== undefined,
  });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ChatComposer, {
    target,
    props: { value: 'A message', sendControl, ...props },
  });
  flushSync();
}

const menuItems = () => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];
// An entry is a label over a one-line description; the label names it.
const itemLabel = (el: Element) => el.querySelector('.flex-col > span')?.textContent;
const itemLabels = () => menuItems().map(itemLabel);
const item = (label: string) => {
  const found = menuItems().find((el) => itemLabel(el) === label);
  if (!found) throw new Error(`no "${label}" menu item`);
  return found;
};
const primaryLabel = () =>
  document.querySelector('.composer-send button')?.textContent?.replace(/\s+/g, ' ').trim();
const isSplit = () => document.querySelector('.composer-send-split') !== null;

describe('ChatComposer send menu', () => {
  afterEach(() => {
    if (component) unmount(component);
    component = null;
    document.body.innerHTML = '';
  });

  it('has no menu on an idle session that cannot start a new session', () => {
    mountComposer({});
    expect(isSplit()).toBe(false);
    expect(menuItems()).toEqual([]);
    expect(primaryLabel()).toBe('Send');
  });

  it('keeps the menu to the two delivery modes without a new-session starter', () => {
    mountComposer({ isLive: true, canSendLive: true });
    expect(isSplit()).toBe(true);
    expect(itemLabels()).toEqual(['Queue', 'Send']);
  });

  it('splits an idle Send to offer New note and New commit, and nothing else', () => {
    mountComposer({ onStartNewSession: vi.fn() });
    expect(isSplit()).toBe(true);
    expect(primaryLabel()).toBe('Send');
    expect(itemLabels()).toEqual(['New note', 'New commit']);
  });

  it('lists the new-session actions below the delivery modes on a live session', () => {
    mountComposer({ isLive: true, canSendLive: true, onStartNewSession: vi.fn() });
    expect(itemLabels()).toEqual(['Queue', 'Send', 'New note', 'New commit']);
  });

  it('offers only the new-session actions when the running session only queues', () => {
    mountComposer({ isLive: true, canSendLive: false, onStartNewSession: vi.fn() });
    expect(primaryLabel()).toBe('Queue');
    expect(itemLabels()).toEqual(['New note', 'New commit']);
  });

  it('starts the chosen kind without sending the message', () => {
    const onStartNewSession = vi.fn();
    const onSend = vi.fn();
    mountComposer({ onStartNewSession, onSend });

    item('New note').click();
    expect(onStartNewSession).toHaveBeenLastCalledWith('note');
    item('New commit').click();
    expect(onStartNewSession).toHaveBeenLastCalledWith('commit');
    expect(onStartNewSession).toHaveBeenCalledTimes(2);
    expect(onSend).not.toHaveBeenCalled();
  });

  it('disables the actions with an empty draft, like Send', () => {
    mountComposer({ value: '   ', onStartNewSession: vi.fn() });
    expect(document.querySelector<HTMLButtonElement>('.composer-send-primary')?.disabled).toBe(
      true
    );
    expect(item('New note').disabled).toBe(true);
    expect(item('New commit').disabled).toBe(true);
  });

  it('disables the actions while a send is in flight', () => {
    mountComposer({ sending: true, onStartNewSession: vi.fn() });
    expect(item('New note').disabled).toBe(true);
    expect(item('New commit').disabled).toBe(true);
  });

  it('disables the actions while there is no session row to act on', () => {
    mountComposer({ sessionBlock: 'loading', onStartNewSession: vi.fn() });
    expect(item('New note').disabled).toBe(true);
    expect(item('New commit').disabled).toBe(true);
  });

  it('disables the actions, but not Send, while the host cannot start a session', () => {
    mountComposer({ onStartNewSession: vi.fn(), newSessionDisabled: true });
    expect(document.querySelector<HTMLButtonElement>('.composer-send-primary')?.disabled).toBe(
      false
    );
    expect(item('New note').disabled).toBe(true);
    expect(item('New commit').disabled).toBe(true);
  });

  it('enables the actions once there is text and the host can start a session', () => {
    mountComposer({ onStartNewSession: vi.fn() });
    expect(item('New note').disabled).toBe(false);
    expect(item('New commit').disabled).toBe(false);
  });
});
