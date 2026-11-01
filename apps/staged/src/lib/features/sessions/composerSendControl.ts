/**
 * composerSendControl.ts — What the chat composer's submit control shows and
 * does, derived from the session's state and the user's chosen mode.
 *
 * A session that is not running has one button: "Send", which starts (or
 * resumes) a turn. A running session queues follow-ups for after the turn —
 * unless the backend reports the session can take a message now (see
 * `SessionLiveDelivery`), in which case the button splits: the primary
 * segment performs the remembered mode (queue or live send) and a dropdown
 * switches between the two. The mode is a global preference
 * (`preferences.composerSendMode`), remembered across sessions and repos.
 */

/** How the composer submits a message while the session is running. */
export type ComposerSendMode = 'queue' | 'send';

export const DEFAULT_COMPOSER_SEND_MODE: ComposerSendMode = 'queue';

/** A stored mode, or the default when the value is missing or malformed. */
export function normalizeComposerSendMode(value: unknown): ComposerSendMode {
  return value === 'queue' || value === 'send' ? value : DEFAULT_COMPOSER_SEND_MODE;
}

/** What pressing the primary segment does. */
export type ComposerSendAction =
  /** Start or resume a turn on a session that is not running. */
  | 'start'
  /** Persist a follow-up for after the running turn ends. */
  | 'queue'
  /** Deliver to the running session now (steer / interrupt-and-reprompt). */
  | 'send-now';

export interface ComposerSendControl {
  /** Whether the mode dropdown is rendered beside the primary segment. */
  split: boolean;
  action: ComposerSendAction;
  /** Primary segment label. */
  label: string;
  /** Hover title for the primary segment. */
  title: string;
  /** Placeholder for the empty input. */
  placeholder: string;
}

export interface ComposerSendControlInput {
  /** The session is running. */
  isLive: boolean;
  /** The backend reports the running session can take a message now. */
  canSendLive: boolean;
  /** The remembered mode (only consulted when both of the above hold). */
  mode: ComposerSendMode;
}

export function composerSendControl({
  isLive,
  canSendLive,
  mode,
}: ComposerSendControlInput): ComposerSendControl {
  if (!isLive) {
    return {
      split: false,
      action: 'start',
      label: 'Send',
      title: 'Send message',
      placeholder: 'Send a message…',
    };
  }
  if (!canSendLive || mode === 'queue') {
    return {
      split: canSendLive,
      action: 'queue',
      label: 'Queue',
      title: 'Queue message for after this turn',
      placeholder: 'Type to queue a follow-up…',
    };
  }
  return {
    split: true,
    action: 'send-now',
    label: 'Send',
    title: 'Send to the running agent now',
    placeholder: 'Send a message to the running agent…',
  };
}

/** Why the submit control is disabled, or `null` when it can be pressed. */
export type ComposerSubmitBlock =
  /** The session row has not loaded yet; the draft can be typed meanwhile. */
  | 'loading'
  /** A send is in flight and owns the composer until it settles. */
  | 'sending'
  /** Nothing to send. */
  | 'empty';

export interface ComposerSubmitBlockInput {
  loading: boolean;
  sending: boolean;
  /** The draft has non-whitespace text. */
  hasText: boolean;
}

/**
 * Whether the composer's submit control is disabled, and why. `loading` wins
 * because it is the only block the label does not already explain (the
 * spinner covers `sending`, the empty box covers `empty`), so it gets a
 * hover title of its own: `COMPOSER_LOADING_TITLE`.
 */
export function composerSubmitBlock({
  loading,
  sending,
  hasText,
}: ComposerSubmitBlockInput): ComposerSubmitBlock | null {
  if (loading) return 'loading';
  if (sending) return 'sending';
  if (!hasText) return 'empty';
  return null;
}

/** Hover title for the submit control while the session row is loading. */
export const COMPOSER_LOADING_TITLE = 'Loading session…';

/** Menu entries for the mode dropdown, in display order. */
export const COMPOSER_SEND_MODE_OPTIONS: readonly {
  mode: ComposerSendMode;
  label: string;
  description: string;
}[] = [
  { mode: 'queue', label: 'Queue', description: 'Deliver after the current turn finishes' },
  { mode: 'send', label: 'Send', description: 'Deliver to the running agent now' },
];
