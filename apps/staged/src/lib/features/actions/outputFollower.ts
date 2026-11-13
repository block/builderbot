/**
 * Joins an execution's buffered output to its live output.
 *
 * A view of an action's output has two sources: `get_action_output_buffer`,
 * a snapshot of what the backend has retained, and `action_output` events,
 * which carry what is written from then on. Neither says by itself where the
 * other stops, and the two travel separately, so an event can arrive on
 * either side of a snapshot that already holds its text.
 *
 * Both are positioned in the same stream of bytes. An event says where its
 * chunk starts (`offset`); a snapshot says where it ends (`endOffset`). The
 * backend appends a batch of output to its buffer before it emits it, and
 * appends it whole, so a snapshot always ends on the boundary between two
 * events. The follower shows a snapshot and then exactly the events that
 * continue it:
 *
 * - an event that ends at or before the end of what is shown is already
 *   there, and is skipped;
 * - an event that starts where what is shown ends is shown;
 * - anything else means output was missed (a dropped event, a reconnect),
 *   and the follower takes a new snapshot rather than show output with a
 *   hole in it.
 *
 * The caller starts listening first and calls `sync()` once the listener is
 * live, and again whenever the event stream says it may have dropped
 * something. Output written before the listener went live is then in the
 * snapshot, and output written after it arrives as events.
 */

import type { ActionOutputEvent, BufferedOutput, OutputChunk } from './actions';

/** Output text and the stream it was written to. */
export type OutputText = Pick<OutputChunk, 'chunk' | 'stream'>;

/** What the follower needs of an `action_output` event. */
export type OutputEvent = Pick<ActionOutputEvent, 'chunk' | 'stream' | 'offset'>;

export interface OutputFollowerOptions {
  /** Take a snapshot of the buffered output. Null when there is none. */
  fetchSnapshot: () => Promise<BufferedOutput | null>;
  /** Show `chunks` in place of everything shown so far. */
  onSnapshot: (chunks: OutputChunk[]) => void;
  /** Show `output` after everything shown so far. */
  onOutput: (output: OutputText) => void;
  /**
   * There is no output to show: the first snapshot found none buffered,
   * because the execution is unknown or its output has been cleared.
   */
  onUnavailable: () => void;
  /** A snapshot could not be taken. What is shown, if anything, stays. */
  onError: (error: unknown) => void;
}

export interface OutputFollower {
  /**
   * Take a snapshot and continue from it. Call once the event listener is
   * live, and again whenever events may have been lost.
   */
  sync(): void;
  /** Feed an `action_output` event of the execution being followed. */
  handleEvent(event: OutputEvent): void;
  /** Stop: nothing is shown or fetched after this. */
  stop(): void;
}

/**
 * Most text held back while a snapshot is in flight. Past this the held
 * events are dropped and another snapshot is taken instead, which is both
 * smaller (the backend retains 4 MiB) and certain to include them.
 */
export const MAX_HELD_CHARS = 4 * 1024 * 1024;

/** The number of bytes `text` takes as UTF-8, which is what offsets count. */
export function utf8Length(text: string): number {
  // One byte per UTF-16 unit, plus what the wider characters add.
  let bytes = text.length;
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code < 0x80) continue;
    if (code < 0x800) {
      bytes += 1;
    } else if (code >= 0xd800 && code <= 0xdbff && isLowSurrogate(text.charCodeAt(i + 1))) {
      // A surrogate pair: two units, four bytes.
      bytes += 2;
      i++;
    } else {
      bytes += 2;
    }
  }
  return bytes;
}

function isLowSurrogate(code: number): boolean {
  return code >= 0xdc00 && code <= 0xdfff;
}

export function createOutputFollower(opts: OutputFollowerOptions): OutputFollower {
  /**
   * Where what is shown ends, and so where the next chunk to show must
   * start. Null until a snapshot has been shown.
   */
  let shownEnd: number | null = null;
  let fetching = false;
  /** Another snapshot is wanted once the one in flight has landed. */
  let fetchAgain = false;
  /** Events that arrived while a snapshot was in flight, in order. */
  let held: OutputEvent[] = [];
  let heldChars = 0;
  /** The output is gone from the backend: there is nothing left to follow. */
  let detached = false;
  let stopped = false;

  function dropHeld(): void {
    held = [];
    heldChars = 0;
  }

  function hold(event: OutputEvent): void {
    held.push(event);
    heldChars += event.chunk.length;
    if (heldChars > MAX_HELD_CHARS) {
      // Some of these may follow the snapshot in flight, so it cannot be
      // trusted to cover them. A later one will.
      dropHeld();
      fetchAgain = true;
    }
  }

  function sync(): void {
    if (stopped || detached) return;
    if (fetching) {
      // The snapshot in flight may have been taken before whatever prompted
      // this call.
      fetchAgain = true;
      return;
    }
    fetching = true;
    void takeSnapshot();
  }

  async function takeSnapshot(): Promise<void> {
    let snapshot: BufferedOutput | null = null;
    let failure: { error: unknown } | null = null;
    try {
      snapshot = await opts.fetchSnapshot();
    } catch (error) {
      failure = { error };
    }
    if (stopped) return;
    fetching = false;

    if (fetchAgain) {
      // Not shown: it may predate lost output, and its successor replaces it.
      // The held events stay; those the next snapshot covers are skipped.
      fetchAgain = false;
      sync();
      return;
    }

    if (failure) {
      // Without a snapshot the held events have nothing to continue. The
      // next sync, or the next event that does not fit, tries again.
      dropHeld();
      opts.onError(failure.error);
      return;
    }

    if (snapshot === null) {
      dropHeld();
      detached = true;
      if (shownEnd === null) opts.onUnavailable();
      return;
    }

    // A snapshot that ends where the view already does holds nothing new, so
    // the view is left as it is instead of being rebuilt.
    if (shownEnd === null || snapshot.endOffset > shownEnd) {
      opts.onSnapshot(snapshot.chunks);
      shownEnd = snapshot.endOffset;
    }

    const events = held;
    dropHeld();
    for (const event of events) handleEvent(event);
  }

  function handleEvent(event: OutputEvent): void {
    if (stopped || detached) return;
    if (fetching) {
      hold(event);
      return;
    }
    // Nothing shown and no snapshot on its way: the event has nothing to
    // continue, and the snapshot that is eventually taken will hold it.
    if (shownEnd === null) return;

    const end = event.offset + utf8Length(event.chunk);
    if (end <= shownEnd) return;
    if (event.offset !== shownEnd) {
      // Output between what is shown and this event never arrived. The
      // snapshot will hold it, and this event too.
      sync();
      return;
    }
    opts.onOutput(event);
    shownEnd = end;
  }

  return {
    sync,
    handleEvent,
    stop() {
      stopped = true;
      dropHeld();
    },
  };
}
