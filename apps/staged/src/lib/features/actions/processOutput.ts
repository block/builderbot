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

/**
 * Incremental line processor that maintains state across calls.
 *
 * Unlike `processChunksToLines` (which reprocesses every chunk from scratch),
 * this only processes *new* chunks each time `process()` is called, appending
 * to an internal list of finalized lines.  This turns the per-update cost from
 * O(total-characters) to O(new-characters).
 */
export function createIncrementalProcessor() {
  const finalizedLines: TerminalLine[] = [];
  let currentText = '';
  let currentStream: 'stdout' | 'stderr' = 'stdout';
  let pendingCR = false;

  return {
    /**
     * Feed new chunks and return the full (finalized + in-progress) line list.
     */
    process(chunks: OutputChunk[]): TerminalLine[] {
      for (const chunk of chunks) {
        const raw = chunk.chunk;
        const stream = chunk.stream;

        for (let i = 0; i < raw.length; i++) {
          const ch = raw[i];

          if (pendingCR) {
            pendingCR = false;
            if (ch === '\n') {
              finalizedLines.push({ text: currentText, stream: currentStream });
              currentText = '';
              currentStream = stream;
              continue;
            } else {
              currentText = '';
              currentStream = stream;
            }
          }

          if (ch === '\n') {
            finalizedLines.push({ text: currentText, stream: currentStream });
            currentText = '';
            currentStream = stream;
          } else if (ch === '\r') {
            if (i + 1 < raw.length && raw[i + 1] === '\n') {
              finalizedLines.push({ text: currentText, stream: currentStream });
              currentText = '';
              currentStream = stream;
              i++;
            } else if (i + 1 < raw.length) {
              currentText = '';
              currentStream = stream;
            } else {
              pendingCR = true;
            }
          } else {
            currentText += ch;
            currentStream = stream;
          }
        }
      }

      if (pendingCR) {
        // Don't discard — just return what we have; the bare CR will resolve
        // on the next call when more data arrives (or on final snapshot).
      }

      if (currentText.length > 0) {
        return [...finalizedLines, { text: currentText, stream: currentStream }];
      }
      return [...finalizedLines];
    },
  };
}
