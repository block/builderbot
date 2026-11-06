import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SessionReconcileEvent } from '../../types';
import DisconnectedSessionAlert from './DisconnectedSessionAlert.svelte';
import { reactiveProps } from './testStubs/reactiveProps.svelte';

function reconcile(
  phase: SessionReconcileEvent['phase'],
  overrides: Partial<SessionReconcileEvent> = {}
): SessionReconcileEvent {
  return {
    sessionId: 's1',
    branchId: null,
    projectId: null,
    phase,
    reason: null,
    attempts: 1,
    nextAttemptAt: null,
    ...overrides,
  };
}

let alert: ReturnType<typeof mount> | null = null;

function render(initial: {
  reconcile: SessionReconcileEvent | null;
  checkRequested?: boolean;
  stopping?: boolean;
  userScrolledUp?: boolean;
}) {
  const onCheckNow = vi.fn();
  const onStop = vi.fn();
  const props = reactiveProps({
    checkRequested: false,
    stopping: false,
    userScrolledUp: false,
    ...initial,
    onCheckNow,
    onStop,
  });
  const target = document.createElement('div');
  document.body.appendChild(target);
  alert = mount(DisconnectedSessionAlert, { target, props });
  flushSync();
  return { props, onCheckNow, onStop };
}

function button(label: string): HTMLButtonElement {
  const found = [...document.querySelectorAll<HTMLButtonElement>('button')].find(
    (b) => b.textContent?.trim() === label
  );
  if (!found) throw new Error(`no ${label} button`);
  return found;
}

const statusLine = () =>
  document.querySelector<HTMLElement>('[data-testid="disconnected-session-status"]');

afterEach(() => {
  if (alert) unmount(alert);
  alert = null;
  document.body.innerHTML = '';
});

describe('DisconnectedSessionAlert', () => {
  it('wires Check now and Stop to their handlers', () => {
    const { onCheckNow, onStop } = render({ reconcile: null });
    expect(statusLine()).toBeNull();

    button('Check now').click();
    button('Stop').click();
    expect(onCheckNow).toHaveBeenCalledTimes(1);
    expect(onStop).toHaveBeenCalledTimes(1);
  });

  it('holds Check now while catching up, leaving Stop available', () => {
    const { props } = render({ reconcile: null, checkRequested: true });
    expect(statusLine()?.textContent?.trim()).toBe('Catching up with the workstation…');
    expect(button('Check now').disabled).toBe(true);
    expect(button('Stop').disabled).toBe(false);

    props.checkRequested = false;
    props.reconcile = reconcile('checking');
    flushSync();
    expect(button('Check now').disabled).toBe(true);
  });

  it('holds both buttons while a stop is in flight, from either side', () => {
    const { props } = render({ reconcile: null, stopping: true });
    expect(button('Check now').disabled).toBe(true);
    expect(button('Stop').disabled).toBe(true);

    props.stopping = false;
    props.reconcile = reconcile('stopping');
    flushSync();
    expect(statusLine()?.textContent?.trim()).toBe('Stopping the turn on the workstation…');
    expect(button('Check now').disabled).toBe(true);
    expect(button('Stop').disabled).toBe(true);
  });

  it('surfaces the reason, with the raw error on hover', () => {
    render({
      reconcile: reconcile('waiting', {
        reason: { kind: 'not_authenticated', message: 'sq: token expired' },
      }),
    });
    expect(statusLine()?.textContent?.trim()).toBe('Staged is signed out of Blox.');
    expect(statusLine()?.title).toBe('sq: token expired');
    expect(button('Check now').disabled).toBe(false);
  });

  function spyScrollIntoView() {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    return scrollIntoView;
  }

  afterEach(() => {
    delete (Element.prototype as { scrollIntoView?: unknown }).scrollIntoView;
  });

  it('scrolls itself into view when it appears, and not for the background checks after', () => {
    const scrollIntoView = spyScrollIntoView();
    const { props } = render({ reconcile: reconcile('waiting') });
    expect(scrollIntoView).toHaveBeenCalledTimes(1);
    expect(scrollIntoView).toHaveBeenLastCalledWith({ block: 'nearest' });
    expect(scrollIntoView.mock.contexts[0]).toBe(
      document.querySelector('[data-testid="disconnected-session-alert"]')
    );

    // A sweep's check: `checking`, then `waiting` with its reason, and the
    // countdown ticking after it. The status line changes each time, and a
    // reader who has scrolled away is left where they are.
    props.reconcile = reconcile('checking');
    flushSync();
    expect(statusLine()?.textContent?.trim()).toBe('Catching up with the workstation…');
    const reason = { kind: 'unreachable' as const, message: 'sq: timed out' };
    props.reconcile = reconcile('waiting', { reason, nextAttemptAt: Date.now() + 10 * 60_000 });
    flushSync();
    expect(statusLine()?.textContent).toMatch(/Next check in \d+ min\./);
    props.reconcile = reconcile('waiting', { reason, nextAttemptAt: Date.now() + 5 * 60_000 });
    flushSync();
    props.reconcile = reconcile('stopping');
    flushSync();
    expect(statusLine()?.textContent?.trim()).toBe('Stopping the turn on the workstation…');
    expect(scrollIntoView).toHaveBeenCalledTimes(1);
  });

  it('follows its status line while the reader’s own Check now plays out', () => {
    const scrollIntoView = spyScrollIntoView();
    const reason = { kind: 'unreachable' as const, message: 'sq: timed out' };
    const { props } = render({ reconcile: reconcile('waiting', { reason }) });
    expect(scrollIntoView).toHaveBeenCalledTimes(1);

    button('Check now').click();
    props.checkRequested = true;
    flushSync();
    expect(statusLine()?.textContent?.trim()).toBe('Catching up with the workstation…');
    expect(scrollIntoView).toHaveBeenCalledTimes(2);

    // The backend's `checking` says what the pane already shows.
    props.checkRequested = false;
    props.reconcile = reconcile('checking');
    flushSync();
    expect(scrollIntoView).toHaveBeenCalledTimes(2);

    // Its outcome, the status line it grows, is brought into view too.
    props.reconcile = reconcile('waiting', {
      reason: { kind: 'suspended', message: 'sq: suspended' },
    });
    flushSync();
    expect(scrollIntoView).toHaveBeenCalledTimes(3);

    // That settled the Check now: the next background check doesn't scroll.
    props.reconcile = reconcile('checking');
    flushSync();
    props.reconcile = reconcile('waiting', { reason });
    flushSync();
    expect(scrollIntoView).toHaveBeenCalledTimes(3);
  });

  it('follows its status line while the reader’s own Stop plays out', () => {
    const scrollIntoView = spyScrollIntoView();
    const { props } = render({ reconcile: reconcile('waiting') });
    expect(scrollIntoView).toHaveBeenCalledTimes(1);

    button('Stop').click();
    props.stopping = true;
    flushSync();
    props.reconcile = reconcile('stopping');
    flushSync();
    expect(scrollIntoView).toHaveBeenCalledTimes(2);
  });

  it('never scrolls a reader who has scrolled up the transcript', () => {
    const scrollIntoView = spyScrollIntoView();
    const { props } = render({ reconcile: reconcile('waiting'), userScrolledUp: true });
    expect(scrollIntoView).not.toHaveBeenCalled();

    button('Check now').click();
    props.checkRequested = true;
    flushSync();
    expect(statusLine()?.textContent?.trim()).toBe('Catching up with the workstation…');
    expect(scrollIntoView).not.toHaveBeenCalled();

    // Scrolling back down is not itself a reason to scroll; the action's
    // next change, now the reader follows the transcript, is.
    props.userScrolledUp = false;
    flushSync();
    expect(scrollIntoView).not.toHaveBeenCalled();
    props.checkRequested = false;
    props.reconcile = reconcile('waiting', {
      reason: { kind: 'suspended', message: 'sq: suspended' },
    });
    flushSync();
    expect(scrollIntoView).toHaveBeenCalledTimes(1);
  });
});
