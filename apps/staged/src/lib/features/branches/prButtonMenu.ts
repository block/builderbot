/**
 * The entries of the PR button's split menu (the chevron joined to it).
 *
 * Pure so the state table can be tested without mounting the button. The
 * component maps each `id` to an icon and a handler; icons stay out of here so
 * tests need no icon mocks. The chevron shows whenever the list is non-empty.
 *
 * Entries come in sections, joined by separators: creating a PR, pushing now,
 * pushing later ("after current work"), and links to the PR. Each section is
 * its own function, so a new push action extends `pushItems` (and its inputs)
 * without touching the others.
 */

import type { DeferredPushName } from '../../types';
import type { PrState } from '../../stores/prState.svelte';
import type { PushState } from '../../stores/pushState.svelte';
import type { PrPushAction } from './prButtonGitState';

export type PrMenuItemId =
  | 'createDraftPr'
  | 'detectExistingPr'
  | 'pushNoVerify'
  | 'forcePushNoVerify'
  | 'deferPush'
  | 'deferForcePush'
  | 'deferPushNoVerify'
  | 'deferForcePushNoVerify'
  | 'cancelDeferredPush'
  | 'viewPr'
  | 'copyPrUrl';

export interface PrMenuItem {
  id: PrMenuItemId;
  label: string;
  /** Selecting it opens the force-push confirm dialog instead of acting. */
  confirm?: boolean;
  /** Shown but not selectable: its action's round trip is still in flight. */
  disabled?: boolean;
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
  /**
   * Whether this branch or the project's own sessions are in flight — what a
   * deferred push waits for (see `deferredPushWaitingMeta`), so the "after
   * current work" rows only show while true.
   */
  workInFlight: boolean;
  /** The deferred push armed on the branch, which the menu offers to cancel. */
  deferredPush: DeferredPushName | null;
  /**
   * An arm or cancel round trip is in flight: the deferred rows stay visible
   * but disabled, so a second selection cannot arm twice or start two
   * immediate pushes.
   */
  deferPushPending: boolean;
}

export function buildPrMenu(input: PrMenuInput): PrMenuEntry[] {
  const sections = [
    createPrItems(input),
    pushItems(input),
    deferredPushItems(input),
    prLinkItems(input),
  ];
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

/** The push a deferred-push name stands for. */
export function deferredPushVariant(name: DeferredPushName): { force: boolean; noVerify: boolean } {
  return {
    force: name === 'force' || name === 'force-no-verify',
    noVerify: name === 'push-no-verify' || name === 'force-no-verify',
  };
}

/** The inverse of `deferredPushVariant`. */
export function deferredPushName(force: boolean, noVerify: boolean): DeferredPushName {
  if (force) return noVerify ? 'force-no-verify' : 'force';
  return noVerify ? 'push-no-verify' : 'push';
}

/** A deferred push's label, as the menu and the timeline's footer row show it. */
export function deferredPushLabel(name: DeferredPushName): string {
  const { force, noVerify } = deferredPushVariant(name);
  return `${force ? 'Force push' : 'Push'} after current work${noVerify ? ' without verifying' : ''}`;
}

/**
 * What a deferred push is waiting on, as this client can see it: this branch's
 * sessions (timeline sessions queued, running or disconnected, plus our own
 * push/pull actions) and the project's own sessions (project-note sessions
 * running or disconnected). Sessions on the project's other branches do not
 * hold the push and are not counted — this mirrors the backend's idle check.
 */
export interface DeferredPushWaitingOn {
  branchSessions: number;
  projectSessions: number;
}

/**
 * The footer row's text for an armed deferred push. Names the counts when the
 * client can see something; the generic form covers what it cannot (a queued
 * project session, or a snapshot this client has not loaded yet).
 */
export function deferredPushWaitingMeta(waitingOn?: DeferredPushWaitingOn | null): string {
  const parts: string[] = [];
  if (waitingOn && waitingOn.branchSessions > 0) {
    parts.push(countNoun(waitingOn.branchSessions, 'branch session'));
  }
  if (waitingOn && waitingOn.projectSessions > 0) {
    parts.push(countNoun(waitingOn.projectSessions, 'project session'));
  }
  if (parts.length === 0) return 'Waiting for branch and project sessions to finish';
  return `Waiting for ${parts.join(' and ')} to finish`;
}

function countNoun(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? '' : 's'}`;
}

/**
 * "Push after current work": a push that fires once the branch and its project
 * are idle. Two rows, one per hook flavour, each Option-swapping to its force
 * variant, as the immediate no-verify row does; in the diverged state the
 * force variants show directly and confirm (unless Option is held), like the
 * button. Offered in the in-sync state too: being in sync while work is in
 * flight is the main use case. While one is armed, the section is just its
 * Cancel row, in every state — the flag can coexist with a queued or running
 * push. The timeline's footer row is the other undo, which is why the plain
 * variants need no confirm. While an arm or cancel round trip is pending the
 * rows stay listed but disabled.
 */
function deferredPushItems(input: PrMenuInput): PrMenuItem[] {
  const items = deferredPushSection(input);
  return input.deferPushPending ? items.map((item) => ({ ...item, disabled: true })) : items;
}

function deferredPushSection({
  prState,
  prStatusState,
  pushState,
  pushAction,
  altHeld,
  workInFlight,
  deferredPush,
}: PrMenuInput): PrMenuItem[] {
  if (deferredPush) return [{ id: 'cancelDeferredPush', label: 'Cancel deferred push' }];
  if (
    prState !== 'created' ||
    prStatusState === 'MERGED' ||
    pushState !== 'idle' ||
    !workInFlight
  ) {
    return [];
  }
  const force: PrMenuItem[] = [
    { id: 'deferForcePush', label: deferredPushLabel('force') },
    { id: 'deferForcePushNoVerify', label: deferredPushLabel('force-no-verify') },
  ];
  if (pushAction === 'forcePush') {
    return altHeld
      ? force
      : force.map((item) => ({ ...item, label: `${item.label}…`, confirm: true }));
  }
  return altHeld
    ? force
    : [
        { id: 'deferPush', label: deferredPushLabel('push') },
        { id: 'deferPushNoVerify', label: deferredPushLabel('push-no-verify') },
      ];
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
