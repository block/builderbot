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
}) {
  const onCheckNow = vi.fn();
  const onStop = vi.fn();
  const props = reactiveProps({
    checkRequested: false,
    stopping: false,
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
});
