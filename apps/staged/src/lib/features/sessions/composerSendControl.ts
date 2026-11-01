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

/** Menu entries for the mode dropdown, in display order. */
export const COMPOSER_SEND_MODE_OPTIONS: readonly {
  mode: ComposerSendMode;
  label: string;
  description: string;
}[] = [
  { mode: 'queue', label: 'Queue', description: 'Deliver after the current turn finishes' },
  { mode: 'send', label: 'Send', description: 'Deliver to the running agent now' },
];
