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

describe('registerTopBar with inactive layers', () => {
  // The home grid and the project view are stacked layers that stay mounted,
  // so a registration carries whether its layer is active; the most recent
  // active one owns the bar (see WorkspaceLayer.svelte / TopBarPortal).

  it('skips inactive registrations and hands the bar over as `active` flips', async () => {
    const { registerTopBar, topBar } = await importTopBarState();

    const home = registerTopBar({ title: 'Projects' });
    expect(topBar.title).toBe('Projects');

    // Opening a project: home's layer goes inactive before the project view
    // mounts, so for a moment nothing active is registered.
    home.update({ title: 'Projects' }, false);
    expect(topBar.title).toBe('');
    const project = registerTopBar({ title: 'Onboarding flow' });
    expect(topBar.title).toBe('Onboarding flow');

    // Going home: the project view unmounts and home's layer is active again.
    project.release();
    home.update({ title: 'Projects' }, true);
    expect(topBar.title).toBe('Projects');
  });

  it('lets a lower active registration show over a later inactive one', async () => {
    const { registerTopBar, topBar } = await importTopBarState();

    const project = registerTopBar({ title: 'Onboarding flow' });
    const home = registerTopBar({ title: 'Projects' }, false);
    expect(topBar.title).toBe('Onboarding flow');

    // Returning to the project with home mounted later: flipping flags, not
    // mount order, decides.
    home.update({ title: 'Projects' }, true);
    project.update({ title: 'Onboarding flow' }, false);
    expect(topBar.title).toBe('Projects');
    home.update({ title: 'Projects' }, false);
    project.update({ title: 'Onboarding flow' }, true);
    expect(topBar.title).toBe('Onboarding flow');
  });

  it('ignores content updates from an inactive registration', async () => {
    const { registerTopBar, topBar } = await importTopBarState();

    const project = registerTopBar({ title: 'Onboarding flow' });
    const home = registerTopBar({ title: 'Projects' }, false);

    home.update({ title: 'Projects (2 unread)' });
    expect(topBar.title).toBe('Onboarding flow');

    // ...but that content is what shows once it is active.
    project.update({ title: 'Onboarding flow' }, false);
    home.update({ title: 'Projects (2 unread)' }, true);
    expect(topBar.title).toBe('Projects (2 unread)');
  });

  it('lets settings registered last win whatever the layer flags say', async () => {
    const { registerTopBar, topBar } = await importTopBarState();

    const home = registerTopBar({ title: 'Projects' }, false);
    const project = registerTopBar({ title: 'Onboarding flow' });
    const settings = registerTopBar({ title: 'Settings' });
    expect(topBar.title).toBe('Settings');

    // Layer flags flipping under settings don't hijack the bar...
    home.update({ title: 'Projects' }, true);
    project.update({ title: 'Onboarding flow' }, false);
    expect(topBar.title).toBe('Settings');

    // ...and releasing settings hands it to whichever layer is active now.
    settings.release();
    expect(topBar.title).toBe('Projects');
  });
});
