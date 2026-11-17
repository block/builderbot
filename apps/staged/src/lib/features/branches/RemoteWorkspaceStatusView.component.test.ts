import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import RemoteWorkspaceStatusView from './RemoteWorkspaceStatusView.svelte';
import type { WorkspaceStatus } from '../../types';

// A remote branch in `error` is only left by setting it up again, so the
// error view offers Retry whenever the card is given a way to do it.

let component: ReturnType<typeof mount> | null = null;
let target: HTMLElement;

function render(props: { status: WorkspaceStatus; workspaceError?: string; onRetry?: () => void }) {
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(RemoteWorkspaceStatusView, { target, props });
  flushSync();
}

function retryButton(): HTMLButtonElement | undefined {
  return [...target.querySelectorAll('button')].find((b) => b.textContent?.trim() === 'Retry');
}

afterEach(() => {
  if (component) unmount(component);
  component = null;
  target.remove();
});

describe('RemoteWorkspaceStatusView', () => {
  it('shows the reason and a Retry that calls onRetry in the error state', () => {
    const onRetry = vi.fn();
    render({ status: 'error', workspaceError: 'could not fetch', onRetry });

    expect(target.textContent).toContain('could not fetch');
    retryButton()?.click();
    flushSync();
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it('has no Retry without onRetry, or outside the error state', () => {
    render({ status: 'error', workspaceError: 'could not fetch' });
    expect(retryButton()).toBeUndefined();
    unmount(component!);
    target.remove();

    render({ status: 'suspended', onRetry: vi.fn() });
    expect(retryButton()).toBeUndefined();
  });
});
