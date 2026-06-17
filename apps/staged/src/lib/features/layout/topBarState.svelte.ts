import type { Snippet } from 'svelte';

export interface TopBarContent {
  title: string;
  subtitle: string | null;
  leading: Snippet | null;
  badges: Snippet | null;
  leftActions: Snippet | null;
  rightActions: Snippet | null;
}

export type TopBarRegistration = Partial<TopBarContent>;

export const topBar = $state<TopBarContent>({
  title: '',
  subtitle: null,
  leading: null,
  badges: null,
  leftActions: null,
  rightActions: null,
});

let activeRegistration = 0;

function applyTopBar(content: TopBarRegistration): void {
  topBar.title = content.title ?? '';
  topBar.subtitle = content.subtitle ?? null;
  topBar.leading = content.leading ?? null;
  topBar.badges = content.badges ?? null;
  topBar.leftActions = content.leftActions ?? null;
  topBar.rightActions = content.rightActions ?? null;
}

function clearTopBar(): void {
  applyTopBar({});
}

export function registerTopBar(content: TopBarRegistration): () => void {
  const registration = ++activeRegistration;
  applyTopBar(content);

  return () => {
    if (activeRegistration === registration) {
      clearTopBar();
    }
  };
}
