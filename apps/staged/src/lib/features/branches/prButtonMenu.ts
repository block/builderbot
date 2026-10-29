/**
 * The entries of the PR button's split menu (the chevron joined to it).
 *
 * Pure so the state table can be tested without mounting the button. The
 * component maps each `id` to an icon and a handler; icons stay out of here so
 * tests need no icon mocks. The chevron shows whenever the list is non-empty.
 *
 * Entries come in sections, joined by separators: creating a PR, pushing, and
 * links to the PR. Each section is its own function, so a new push action
 * extends `pushItems` (and its inputs) without touching the others.
 */

import type { PrState } from '../../stores/prState.svelte';
import type { PushState } from '../../stores/pushState.svelte';
import type { PrPushAction } from './prButtonGitState';

export type PrMenuItemId =
  | 'createDraftPr'
  | 'detectExistingPr'
  | 'pushNoVerify'
  | 'forcePushNoVerify'
  | 'viewPr'
  | 'copyPrUrl';

export interface PrMenuItem {
  id: PrMenuItemId;
  label: string;
  /** Selecting it opens the force-push confirm dialog instead of acting. */
  confirm?: boolean;
}

export interface PrMenuSeparator {
  id: 'separator';
}

export type PrMenuEntry = PrMenuItem | PrMenuSeparator;

export interface PrMenuInput {
  prState: PrState;
  /** GitHub's state for the PR ("OPEN", "CLOSED", "MERGED"), when known. */
  prStatusState: string | null;
  pushState: PushState;
  pushAction: PrPushAction;
  prNumber: number | null;
  /** Whether Option is held right now, which swaps in the force variants. */
  altHeld: boolean;
}

export function buildPrMenu(input: PrMenuInput): PrMenuEntry[] {
  const sections = [createPrItems(input), pushItems(input), prLinkItems(input)];
  const entries: PrMenuEntry[] = [];
  for (const section of sections) {
    if (section.length === 0) continue;
    if (entries.length > 0) entries.push({ id: 'separator' });
    entries.push(...section);
  }
  return entries;
}

/**
 * Whether a click on the PR button itself opens the PR, mirroring its click
 * handler: with a PR, it does unless a push is queued, running or failed, or
 * it is offering one.
 */
export function primaryOpensPr({
  prState,
  pushState,
  pushAction,
}: Pick<PrMenuInput, 'prState' | 'pushState' | 'pushAction'>): boolean {
  if (prState !== 'created') return false;
  if (pushState === 'done') return true;
  return pushState === 'idle' && pushAction === 'none';
}

/** While the button reads "Create PR". */
function createPrItems({ prState, pushState }: PrMenuInput): PrMenuItem[] {
  if (prState !== 'idle' || pushState !== 'idle') return [];
  return [
    { id: 'createDraftPr', label: 'Create draft PR' },
    { id: 'detectExistingPr', label: 'Detect existing PR' },
  ];
}

/**
 * The "without verifying" (`--no-verify`) form of the push the button offers.
 *
 * Only one variant shows per state, because the other one would be redundant
 * (a force push of a fast-forward) or would fail (a plain push onto a diverged
 * remote). Option swaps in the force variant and skips its confirm dialog, the
 * same as it does on the button. On mobile, without Option, the variant that
 * works is still listed.
 */
function pushItems({
  prState,
  prStatusState,
  pushState,
  pushAction,
  altHeld,
}: PrMenuInput): PrMenuItem[] {
  if (prState !== 'created' || prStatusState === 'MERGED' || pushState !== 'idle') return [];
  switch (pushAction) {
    case 'push':
      return [
        altHeld
          ? { id: 'forcePushNoVerify', label: 'Force push without verifying' }
          : { id: 'pushNoVerify', label: 'Push without verifying' },
      ];
    case 'forcePush':
      return [
        altHeld
          ? { id: 'forcePushNoVerify', label: 'Force push without verifying' }
          : { id: 'forcePushNoVerify', label: 'Force push without verifying…', confirm: true },
      ];
    case 'none':
      return [];
  }
}

/** View PR (unless the button already opens it) and Copy PR URL. */
function prLinkItems(input: PrMenuInput): PrMenuItem[] {
  if (input.prState !== 'created') return [];
  const items: PrMenuItem[] = [];
  if (!primaryOpensPr(input)) {
    items.push({
      id: 'viewPr',
      label: input.prNumber ? `View PR #${input.prNumber}` : 'View PR',
    });
  }
  items.push({ id: 'copyPrUrl', label: 'Copy PR URL' });
  return items;
}
