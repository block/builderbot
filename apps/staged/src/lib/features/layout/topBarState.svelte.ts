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
  /** Replace this registration's content in place; shows only while it is the top one. */
  update(content: TopBarRegistration): void;
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

// Views register in mount order and the most recent one owns the bar. The
// workspace views stay mounted (parked) under settings and the diff view, so
// a plain "latest wins, clear on release" slot would blank the bar when the
// view on top goes away; a stack hands it back to the view underneath.
let registrations: { content: TopBarRegistration }[] = [];

function applyTopBar(content: TopBarRegistration): void {
  topBar.title = content.title ?? '';
  topBar.subtitle = content.subtitle ?? null;
  topBar.leading = content.leading ?? null;
  topBar.center = content.center ?? null;
  topBar.badges = content.badges ?? null;
  topBar.leftActions = content.leftActions ?? null;
  topBar.rightActions = content.rightActions ?? null;
}

function syncTopBar(): void {
  applyTopBar(registrations[registrations.length - 1]?.content ?? {});
}

export function registerTopBar(content: TopBarRegistration): TopBarHandle {
  const registration = { content };
  registrations.push(registration);
  syncTopBar();

  return {
    update(next) {
      registration.content = next;
      if (registrations[registrations.length - 1] === registration) syncTopBar();
    },
    release() {
      registrations = registrations.filter((r) => r !== registration);
      syncTopBar();
    },
  };
}
