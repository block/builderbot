import { describe, it, expect } from 'vitest';
import { processChunksToLines, createIncrementalProcessor } from './processOutput';
import type { OutputChunk } from './actions';

/** Helper to build an OutputChunk. */
function chunk(text: string, stream: 'stdout' | 'stderr' = 'stdout'): OutputChunk {
  return { chunk: text, stream, timestamp: 0 };
}

/** Helper to extract just the text from result lines. */
function texts(chunks: OutputChunk[]): string[] {
  return processChunksToLines(chunks).map((l) => l.text);
}

describe('processChunksToLines', () => {
  // ---------------------------------------------------------------------------
  // Basic newline handling
  // ---------------------------------------------------------------------------

  it('splits on \\n', () => {
    expect(texts([chunk('hello\nworld\n')])).toEqual(['hello', 'world']);
  });

  it('keeps a trailing line that has no terminating newline', () => {
    expect(texts([chunk('hello\nworld')])).toEqual(['hello', 'world']);
  });

  it('handles empty input', () => {
    expect(texts([])).toEqual([]);
  });

  it('handles a single chunk with no newlines', () => {
    expect(texts([chunk('hello')])).toEqual(['hello']);
  });

  // ---------------------------------------------------------------------------
  // \\r\\n (CRLF) handling
  // ---------------------------------------------------------------------------

  it('treats \\r\\n as a single newline', () => {
    expect(texts([chunk('hello\r\nworld\r\n')])).toEqual(['hello', 'world']);
  });

  it('handles \\r\\n split across two chunks', () => {
    expect(texts([chunk('hello\r'), chunk('\nworld')])).toEqual(['hello', 'world']);
  });

  // ---------------------------------------------------------------------------
  // Bare \\r (carriage return) — progress bar behavior
  // ---------------------------------------------------------------------------

  it('bare \\r overwrites the current line (single chunk)', () => {
    expect(texts([chunk('progress 50%\rprogress 100%\n')])).toEqual(['progress 100%']);
  });

  it('bare \\r overwrites across multiple updates in one chunk', () => {
    expect(texts([chunk('10%\r20%\r30%\r40%\n')])).toEqual(['40%']);
  });

  it('bare \\r overwrites across separate chunks', () => {
    expect(texts([chunk('downloading 50%\r'), chunk('downloading 100%\n')])).toEqual([
      'downloading 100%',
    ]);
  });

  it('bare \\r at end of chunk followed by non-\\n text in next chunk overwrites', () => {
    expect(texts([chunk('old text\r'), chunk('new text')])).toEqual(['new text']);
  });

  it('multiple progress updates across separate chunks collapse correctly', () => {
    expect(texts([chunk('10%\r'), chunk('20%\r'), chunk('30%\r'), chunk('done\n')])).toEqual([
      'done',
    ]);
  });

  it('progress bar followed by normal output', () => {
    expect(texts([chunk('building...\rprogress 50%\rprogress 100%\nSuccess!\n')])).toEqual([
      'progress 100%',
      'Success!',
    ]);
  });

  // ---------------------------------------------------------------------------
  // Mixed scenarios
  // ---------------------------------------------------------------------------

  it('handles normal lines before and after progress bars', () => {
    expect(
      texts([chunk('Starting build\n'), chunk('0%\r50%\r100%\n'), chunk('Build complete\n')])
    ).toEqual(['Starting build', '100%', 'Build complete']);
  });

  it('handles interleaved stdout and stderr', () => {
    const result = processChunksToLines([
      chunk('out line\n', 'stdout'),
      chunk('err line\n', 'stderr'),
    ]);
    expect(result).toEqual([
      { text: 'out line', stream: 'stdout' },
      { text: 'err line', stream: 'stderr' },
    ]);
  });

  it('bare \\r at the very end of all chunks produces no trailing line', () => {
    // A progress bar that never finishes with \n — should show nothing
    // because the \r resets the line and there's no further content.
    expect(texts([chunk('in progress\r')])).toEqual([]);
  });

  it('empty lines are preserved', () => {
    expect(texts([chunk('a\n\nb\n')])).toEqual(['a', '', 'b']);
  });

  it('handles chunk boundaries mid-text', () => {
    expect(texts([chunk('hel'), chunk('lo\nwor'), chunk('ld\n')])).toEqual(['hello', 'world']);
  });
});

// ---------------------------------------------------------------------------
// createIncrementalProcessor
// ---------------------------------------------------------------------------

describe('createIncrementalProcessor', () => {
  /** Helper: feed chunks one-at-a-time and return the final snapshot. */
  function feedOneByOne(chunks: OutputChunk[]): string[] {
    const proc = createIncrementalProcessor();
    let result: ReturnType<typeof proc.process> = [];
    for (const c of chunks) {
      result = proc.process([c]);
    }
    return result.map((l) => l.text);
  }

  /** Helper: feed all chunks at once. */
  function feedAll(chunks: OutputChunk[]): string[] {
    const proc = createIncrementalProcessor();
    return proc.process(chunks).map((l) => l.text);
  }

  // ---------------------------------------------------------------------------
  // Parity with processChunksToLines
  // ---------------------------------------------------------------------------

  it('matches batch output for simple newlines', () => {
    const chunks = [chunk('hello\nworld\n')];
    expect(feedAll(chunks)).toEqual(texts(chunks));
  });

  it('matches batch output for \\r\\n', () => {
    const chunks = [chunk('hello\r\nworld\r\n')];
    expect(feedAll(chunks)).toEqual(texts(chunks));
  });

  it('matches batch output for bare \\r overwrites', () => {
    const chunks = [chunk('10%\r20%\r30%\r40%\n')];
    expect(feedAll(chunks)).toEqual(texts(chunks));
  });

  it('matches batch output for interleaved stdout/stderr', () => {
    const chunks = [chunk('out\n', 'stdout'), chunk('err\n', 'stderr')];
    const proc = createIncrementalProcessor();
    let result = proc.process([chunks[0]]);
    result = proc.process([chunks[1]]);
    expect(result.map((l) => l.stream)).toEqual(['stdout', 'stderr']);
  });

  // ---------------------------------------------------------------------------
  // Incremental accumulation
  // ---------------------------------------------------------------------------

  it('accumulates lines across multiple process() calls', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('line1\n')]);
    const result = proc.process([chunk('line2\n')]);
    expect(result.map((l) => l.text)).toEqual(['line1', 'line2']);
  });

  it('shows in-progress line until finalized', () => {
    const proc = createIncrementalProcessor();
    let result = proc.process([chunk('partial')]);
    expect(result.map((l) => l.text)).toEqual(['partial']);
    result = proc.process([chunk(' more\n')]);
    expect(result.map((l) => l.text)).toEqual(['partial more']);
  });

  // ---------------------------------------------------------------------------
  // \\r\\n split across process() calls — the key correctness case
  // ---------------------------------------------------------------------------

  it('preserves line text when \\r\\n is split across calls', () => {
    const proc = createIncrementalProcessor();
    // First call ends with \r — we don't yet know if it's bare \r or \r\n
    let result = proc.process([chunk('hello\r')]);
    // The text should still be visible (not discarded)
    expect(result.map((l) => l.text)).toEqual(['hello']);
    // Second call starts with \n — confirms it was \r\n, line should be finalized
    result = proc.process([chunk('\nworld\n')]);
    expect(result.map((l) => l.text)).toEqual(['hello', 'world']);
  });

  it('bare \\r across calls correctly overwrites when followed by non-\\n', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('old text\r')]);
    const result = proc.process([chunk('new text\n')]);
    expect(result.map((l) => l.text)).toEqual(['new text']);
  });

  it('multiple trailing \\r across calls resolve correctly', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('10%\r')]);
    proc.process([chunk('20%\r')]);
    proc.process([chunk('30%\r')]);
    const result = proc.process([chunk('done\n')]);
    expect(result.map((l) => l.text)).toEqual(['done']);
  });

  // ---------------------------------------------------------------------------
  // Batched chunks (multiple chunks in one process() call)
  // ---------------------------------------------------------------------------

  it('handles multiple chunks in a single process() call', () => {
    const proc = createIncrementalProcessor();
    const result = proc.process([chunk('hello\n'), chunk('world\n')]);
    expect(result.map((l) => l.text)).toEqual(['hello', 'world']);
  });
});
