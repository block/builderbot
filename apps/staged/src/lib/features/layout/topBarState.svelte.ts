import type { Snippet } from 'svelte';

export interface TopBarContent {
  title: string;
  subtitle: string | null;
  leading: Snippet | null;
  center: Snippet | null;
  badges: Snippet | null;
  leftActions: Snippet | null;
  rightActions: Snippet | null;
}

export type TopBarRegistration = Partial<TopBarContent>;

export interface TopBarHandle {
  /**
   * Replace this registration's content (and whether its view is active) in
   * place; shows only while it is the top active one.
   */
  update(content: TopBarRegistration, active?: boolean): void;
  release(): void;
}

export const topBar = $state<TopBarContent>({
  title: '',
  subtitle: null,
  leading: null,
  center: null,
  badges: null,
  leftActions: null,
  rightActions: null,
});

// Views register in mount order and the most recent active one owns the bar.
// The workspace views stay mounted (parked) under settings and the diff view,
// so a plain "latest wins, clear on release" slot would blank the bar when the
// view on top goes away; a stack hands it back to the view underneath. Within
// the workspace, the home grid and the project view are stacked layers that
// stay mounted too, so mount order alone can't say which is on screen: a
// registration from an inactive layer is skipped until its layer is active
// again (App.svelte, WorkspaceLayer.svelte).
let registrations: { content: TopBarRegistration; active: boolean }[] = [];

function applyTopBar(content: TopBarRegistration): void {
  topBar.title = content.title ?? '';
  topBar.subtitle = content.subtitle ?? null;
  topBar.leading = content.leading ?? null;
  topBar.center = content.center ?? null;
  topBar.badges = content.badges ?? null;
  topBar.leftActions = content.leftActions ?? null;
  topBar.rightActions = content.rightActions ?? null;
}

function owner(): { content: TopBarRegistration; active: boolean } | undefined {
  for (let i = registrations.length - 1; i >= 0; i--) {
    if (registrations[i].active) return registrations[i];
  }
  return undefined;
}

function syncTopBar(): void {
  applyTopBar(owner()?.content ?? {});
}

export function registerTopBar(content: TopBarRegistration, active = true): TopBarHandle {
  const registration = { content, active };
  registrations.push(registration);
  syncTopBar();

  return {
    update(next, nextActive = registration.active) {
      const wasOwner = owner() === registration;
      registration.content = next;
      registration.active = nextActive;
      // Re-sync when this registration was showing or is now: a flip to
      // inactive hands the bar to the one below, a flip to active takes it.
      if (wasOwner || owner() === registration) syncTopBar();
    },
    release() {
      registrations = registrations.filter((r) => r !== registration);
      syncTopBar();
    },
  };
}
