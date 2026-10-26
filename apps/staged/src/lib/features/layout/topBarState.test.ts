import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// The workspace views stay mounted (parked) under settings and the diff view,
// so the top bar is a stack of registrations rather than a last-wins slot:
// the view on top owns the bar, and releasing it hands the bar back to the
// view underneath (see topBarState.svelte.ts).

async function importTopBarState() {
  return await import('./topBarState.svelte');
}

beforeEach(() => {
  vi.resetModules();
  // Runes compile away in the app build; under vitest they stay plain global
  // calls, so stub $state as identity (navigation.test.ts precedent).
  vi.stubGlobal('$state', (initial: unknown) => initial);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('registerTopBar', () => {
  it('lets a later registration cover an earlier one and hands the bar back on release', async () => {
    const { registerTopBar, topBar } = await importTopBarState();

    registerTopBar({ title: 'Projects' });
    expect(topBar.title).toBe('Projects');

    const settings = registerTopBar({ title: 'Settings', subtitle: 'v1' });
    expect(topBar.title).toBe('Settings');
    expect(topBar.subtitle).toBe('v1');

    settings.release();
    expect(topBar.title).toBe('Projects');
    expect(topBar.subtitle).toBeNull();
  });

  it('ignores updates to a registration that is not on top', async () => {
    const { registerTopBar, topBar } = await importTopBarState();

    const workspace = registerTopBar({ title: 'Onboarding flow' });
    const settings = registerTopBar({ title: 'Settings' });

    // A parked view re-rendering its title must not hijack the bar...
    workspace.update({ title: 'Onboarding flow (3)' });
    expect(topBar.title).toBe('Settings');

    // ...but the new content is what comes back once it is on top again.
    settings.release();
    expect(topBar.title).toBe('Onboarding flow (3)');

    // Updates to the top registration apply immediately.
    workspace.update({ title: 'Onboarding flow (4)' });
    expect(topBar.title).toBe('Onboarding flow (4)');
  });

  it('keeps the top registration when a lower one is released first', async () => {
    const { registerTopBar, topBar } = await importTopBarState();

    const home = registerTopBar({ title: 'Projects' });
    const project = registerTopBar({ title: 'Onboarding flow' });
    const settings = registerTopBar({ title: 'Settings' });

    project.release();
    expect(topBar.title).toBe('Settings');

    settings.release();
    expect(topBar.title).toBe('Projects');

    home.release();
    expect(topBar.title).toBe('');
  });
});
