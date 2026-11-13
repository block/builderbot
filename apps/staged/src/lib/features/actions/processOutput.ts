/**
 * Terminal output processing utilities.
 *
 * Converts raw output chunks (as received from the backend) into display-ready
 * terminal lines, handling carriage returns the way a real terminal would.
 */

import type { OutputChunk } from './actions';

/** A processed terminal line ready for display. */
export interface TerminalLine {
  text: string;
  stream: 'stdout' | 'stderr';
}

/**
 * Process raw output chunks into terminal lines, handling carriage returns.
 *
 * Terminal programs use \r (carriage return without newline) to overwrite the
 * current line in-place — e.g. for progress bars. This function simulates
 * that behavior:
 *   - \n finalizes the current line and starts a new one
 *   - \r\n is treated as a single newline
 *   - \r (not followed by \n) resets the cursor to the start of the current
 *     line so subsequent text overwrites it
 *
 * Because chunks arrive in arbitrary byte boundaries, \r\n may be split across
 * two consecutive chunks. We track this with `pendingCR` so the \r at the end
 * of one chunk and the \n at the start of the next are still treated as a
 * single newline.
 */
export function processChunksToLines(chunks: OutputChunk[]): TerminalLine[] {
  const lines: TerminalLine[] = [];
  let currentText = '';
  let currentStream: 'stdout' | 'stderr' = 'stdout';
  let pendingCR = false;

  for (const chunk of chunks) {
    const raw = chunk.chunk;
    const stream = chunk.stream;

    for (let i = 0; i < raw.length; i++) {
      const ch = raw[i];

      if (pendingCR) {
        pendingCR = false;
        if (ch === '\n') {
          // \r\n split across chunks — treat as a single newline
          lines.push({ text: currentText, stream: currentStream });
          currentText = '';
          currentStream = stream;
          continue;
        } else {
          // The previous \r was a bare carriage return — reset the line
          currentText = '';
          currentStream = stream;
        }
      }

      if (ch === '\n') {
        lines.push({ text: currentText, stream: currentStream });
        currentText = '';
        currentStream = stream;
      } else if (ch === '\r') {
        if (i + 1 < raw.length && raw[i + 1] === '\n') {
          // \r\n within the same chunk
          lines.push({ text: currentText, stream: currentStream });
          currentText = '';
          currentStream = stream;
          i++; // skip the \n
        } else if (i + 1 < raw.length) {
          // Bare \r with more data in this chunk: reset cursor (overwrite)
          currentText = '';
          currentStream = stream;
        } else {
          // \r at the very end of the chunk — defer decision until next chunk
          pendingCR = true;
        }
      } else {
        currentText += ch;
        currentStream = stream;
      }
    }
  }

  // If the last chunk ended with a bare \r that was never resolved, treat it
  // as a carriage return (reset the line).
  if (pendingCR) {
    currentText = '';
  }

  // Don't forget the last in-progress line
  if (currentText.length > 0) {
    lines.push({ text: currentText, stream: currentStream });
  }

  return lines;
}

/** A line as the output view holds it. */
export interface OutputLine extends TerminalLine {
  /**
   * The line's position in the output the processor has been fed, counting
   * the lines it no longer holds. A line keeps its id from the moment it
   * starts until it is dropped, so the id can key a rendered list.
   */
  id: number;
}

/** How much output a processor keeps. */
export interface LineLimits {
  /** Most lines kept. The oldest go first. */
  maxLines: number;
  /** Most characters kept across all lines. The oldest lines go first. */
  maxChars: number;
  /** Longest a single line may grow. What follows on that line is cut. */
  maxLineChars: number;
}

/**
 * The view's limits. The backend retains at most 4 MiB of an execution's
 * output, which is never more than this many characters, so a snapshot loses
 * lines only to the line count.
 */
export const OUTPUT_LINE_LIMITS: LineLimits = {
  maxLines: 5_000,
  maxChars: 4 * 1024 * 1024,
  maxLineChars: 64 * 1024,
};

/** Ends a line that was cut at `maxLineChars`. */
export const LINE_CUT_MARK = ' … [line cut]';

const LF = 10;
const CR = 13;

export interface LineProcessor {
  /**
   * The lines kept, oldest first, ending with the line still being written
   * if there is one. This is the same array on every read: `process` updates
   * it in place rather than building a new one, so the cost of an update does
   * not grow with the output already shown. A line object is replaced, never
   * changed, when its text changes, and is otherwise left alone: a renderer
   * that compares line objects re-renders exactly the lines that changed.
   */
  readonly lines: OutputLine[];
  /** How many lines have been dropped from the front to stay within the limits. */
  readonly droppedLines: number;
  /** Feed the output that follows what has been fed so far. */
  process(chunks: Iterable<Pick<OutputChunk, 'chunk' | 'stream'>>): void;
}

/**
 * Incremental line processor that maintains state across calls.
 *
 * Unlike `processChunksToLines` (which reprocesses every chunk from scratch),
 * this only processes *new* chunks each time `process()` is called, and it
 * keeps a bounded tail of the result: the cost of a call is O(new characters)
 * and what it holds never exceeds `limits`, however long the output runs.
 */
export function createIncrementalProcessor(limits: LineLimits = OUTPUT_LINE_LIMITS): LineProcessor {
  const maxLines = Math.max(1, limits.maxLines);
  const lines: OutputLine[] = [];
  /** Characters in the finished lines in `lines`. */
  let finishedChars = 0;
  let droppedLines = 0;
  /** The id of the line being written, and of the next line to be finished. */
  let nextId = 0;
  let currentText = '';
  let currentStream: 'stdout' | 'stderr' = 'stdout';
  let pendingCR = false;
  /** The line being written reached `maxLineChars`; the rest of it is skipped. */
  let cut = false;
  /** The line being written, as last put at the end of `lines`. */
  let unfinished: OutputLine | null = null;

  function startLine(stream: 'stdout' | 'stderr'): void {
    currentText = '';
    currentStream = stream;
    cut = false;
  }

  function append(text: string, stream: 'stdout' | 'stderr'): void {
    currentStream = stream;
    if (cut) return;
    let room = limits.maxLineChars - currentText.length;
    if (text.length <= room) {
      currentText += text;
      return;
    }
    // Do not leave half of a surrogate pair at the cut.
    const last = room > 0 ? text.charCodeAt(room - 1) : 0;
    if (last >= 0xd800 && last <= 0xdbff) room--;
    currentText += text.slice(0, Math.max(0, room)) + LINE_CUT_MARK;
    cut = true;
  }

  /**
   * Drop lines from the front until the finished lines, plus the unfinished
   * one described by the arguments, are within the limits.
   */
  function trim(unfinishedLines: number, unfinishedChars: number): void {
    let drop = Math.max(0, lines.length + unfinishedLines - maxLines);
    let freed = 0;
    for (let i = 0; i < drop; i++) freed += lines[i].text.length;
    while (drop < lines.length && finishedChars - freed + unfinishedChars > limits.maxChars) {
      freed += lines[drop].text.length;
      drop++;
    }
    if (drop === 0) return;
    lines.splice(0, drop);
    finishedChars -= freed;
    droppedLines += drop;
  }

  function finishLine(nextStream: 'stdout' | 'stderr'): void {
    // A line that was already on show unfinished, and ends as it stood, stays
    // the same object, so it is not rendered again.
    const shown = unfinished;
    const line =
      shown && shown.id === nextId && shown.text === currentText && shown.stream === currentStream
        ? shown
        : { id: nextId, text: currentText, stream: currentStream };
    nextId++;
    lines.push(line);
    finishedChars += line.text.length;
    startLine(nextStream);
    // A large snapshot arrives in one call. Trimming as it goes keeps what
    // is held in the meantime to twice the limit, not the whole snapshot.
    if (lines.length >= 2 * maxLines) trim(0, 0);
  }

  function feed(raw: string, stream: 'stdout' | 'stderr'): void {
    const length = raw.length;
    if (length === 0) return;

    let i = 0;
    if (pendingCR) {
      pendingCR = false;
      if (raw.charCodeAt(0) === LF) {
        // \r\n split across chunks — treat as a single newline
        finishLine(stream);
        i = 1;
      } else {
        // The previous \r was a bare carriage return — reset the line
        startLine(stream);
      }
    }

    // Text between line breaks is taken a run at a time, not a character
    // at a time.
    let runStart = i;
    for (; i < length; i++) {
      const code = raw.charCodeAt(i);
      if (code !== LF && code !== CR) continue;

      if (i > runStart) append(raw.slice(runStart, i), stream);
      if (code === LF) {
        finishLine(stream);
      } else if (i + 1 >= length) {
        // \r at the very end of the chunk — defer decision until next chunk
        pendingCR = true;
      } else if (raw.charCodeAt(i + 1) === LF) {
        // \r\n within the same chunk
        finishLine(stream);
        i++; // skip the \n
      } else {
        // Bare \r with more data in this chunk: reset cursor (overwrite)
        startLine(stream);
      }
      runStart = i + 1;
    }
    if (length > runStart) append(raw.slice(runStart), stream);
  }

  return {
    lines,
    get droppedLines() {
      return droppedLines;
    },
    process(chunks) {
      // The unfinished line comes off the end while output is fed, and goes
      // back on, as it then stands, afterwards.
      if (unfinished) lines.pop();

      for (const chunk of chunks) feed(chunk.chunk, chunk.stream);

      // A pending \r is not resolved here: the line stays on show until the
      // next chunk says whether it was the start of a \r\n or an overwrite.
      const hasUnfinished = currentText.length > 0;
      trim(hasUnfinished ? 1 : 0, currentText.length);
      if (!hasUnfinished) {
        unfinished = null;
        return;
      }
      if (
        !unfinished ||
        unfinished.id !== nextId ||
        unfinished.text !== currentText ||
        unfinished.stream !== currentStream
      ) {
        unfinished = { id: nextId, text: currentText, stream: currentStream };
      }
      lines.push(unfinished);
    },
  };
}
