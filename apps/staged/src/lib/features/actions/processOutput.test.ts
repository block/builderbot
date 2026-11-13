import { describe, it, expect } from 'vitest';
import {
  LINE_CUT_MARK,
  OUTPUT_LINE_LIMITS,
  createIncrementalProcessor,
  processChunksToLines,
  type LineLimits,
} from './processOutput';
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
  /** Helper: feed chunks one-at-a-time and return the final lines. */
  function feedOneByOne(chunks: OutputChunk[]): string[] {
    const proc = createIncrementalProcessor();
    for (const c of chunks) {
      proc.process([c]);
    }
    return proc.lines.map((l) => l.text);
  }

  /** Helper: feed all chunks at once. */
  function feedAll(chunks: OutputChunk[]): string[] {
    const proc = createIncrementalProcessor();
    proc.process(chunks);
    return proc.lines.map((l) => l.text);
  }

  /** Limits that only the named ones can reach. */
  function limits(overrides: Partial<LineLimits>): LineLimits {
    return { maxLines: 1_000_000, maxChars: 1_000_000_000, maxLineChars: 1_000_000, ...overrides };
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
    proc.process([chunks[0]]);
    proc.process([chunks[1]]);
    expect(proc.lines.map((l) => l.stream)).toEqual(['stdout', 'stderr']);
  });

  it('matches batch output however the output is cut into chunks and calls', () => {
    // A fixed pseudo-random sequence, so a failure reproduces.
    let seed = 0x2f6e2b1;
    const random = (below: number) => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed % below;
    };
    const pieces = ['a', 'bc', '\n', '\r', '\r\n', '\n\n', '\r\r', 'x\ry', ' 50%', '\rdone', ''];

    for (let round = 0; round < 300; round++) {
      const chunks: OutputChunk[] = [];
      for (let n = random(12); n >= 0; n--) {
        let text = '';
        for (let m = random(5); m >= 0; m--) text += pieces[random(pieces.length)];
        chunks.push(chunk(text, random(4) === 0 ? 'stderr' : 'stdout'));
      }

      // The batch function resolves a trailing \r as an overwrite; the
      // incremental one keeps the line until more output decides, so the
      // comparison ends on a line break.
      chunks.push(chunk('\n'));
      const expected = processChunksToLines(chunks);

      const proc = createIncrementalProcessor();
      for (let at = 0; at < chunks.length;) {
        const take = 1 + random(3);
        proc.process(chunks.slice(at, at + take));
        at += take;
      }
      const actual = proc.lines.map(({ text, stream }) => ({ text, stream }));
      expect(actual, JSON.stringify(chunks)).toEqual(expected);
    }
  });

  // ---------------------------------------------------------------------------
  // Incremental accumulation
  // ---------------------------------------------------------------------------

  it('accumulates lines across multiple process() calls', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('line1\n')]);
    proc.process([chunk('line2\n')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['line1', 'line2']);
  });

  it('shows in-progress line until finalized', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('partial')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['partial']);
    proc.process([chunk(' more\n')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['partial more']);
  });

  // ---------------------------------------------------------------------------
  // \\r\\n split across process() calls — the key correctness case
  // ---------------------------------------------------------------------------

  it('preserves line text when \\r\\n is split across calls', () => {
    const proc = createIncrementalProcessor();
    // First call ends with \r — we don't yet know if it's bare \r or \r\n
    proc.process([chunk('hello\r')]);
    // The text should still be visible (not discarded)
    expect(proc.lines.map((l) => l.text)).toEqual(['hello']);
    // Second call starts with \n — confirms it was \r\n, line should be finalized
    proc.process([chunk('\nworld\n')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['hello', 'world']);
  });

  it('bare \\r across calls correctly overwrites when followed by non-\\n', () => {
    expect(feedOneByOne([chunk('old text\r'), chunk('new text\n')])).toEqual(['new text']);
  });

  it('multiple trailing \\r across calls resolve correctly', () => {
    expect(feedOneByOne([chunk('10%\r'), chunk('20%\r'), chunk('30%\r'), chunk('done\n')])).toEqual(
      ['done']
    );
  });

  // ---------------------------------------------------------------------------
  // Batched chunks (multiple chunks in one process() call)
  // ---------------------------------------------------------------------------

  it('handles multiple chunks in a single process() call', () => {
    expect(feedAll([chunk('hello\n'), chunk('world\n')])).toEqual(['hello', 'world']);
  });

  // ---------------------------------------------------------------------------
  // Lines are updated in place, not copied
  // ---------------------------------------------------------------------------

  it('hands out one array and updates it in place', () => {
    const proc = createIncrementalProcessor();
    const lines = proc.lines;
    proc.process([chunk('one\n')]);
    proc.process([chunk('two\nthree')]);

    expect(proc.lines).toBe(lines);
    expect(lines.map((l) => l.text)).toEqual(['one', 'two', 'three']);
  });

  it('leaves the objects of finished lines alone', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('one\ntwo\n')]);
    const [one, two] = proc.lines;

    proc.process([chunk('three\nfour')]);
    proc.process([chunk(' more\n')]);

    expect(proc.lines[0]).toBe(one);
    expect(proc.lines[1]).toBe(two);
    expect(one).toEqual({ id: 0, text: 'one', stream: 'stdout' });
  });

  it('replaces the unfinished line when its text changes, and only then', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('done\n10%')]);
    const first = proc.lines[1];

    // Output that does not change the line leaves its object alone...
    proc.process([chunk('')]);
    proc.process([]);
    expect(proc.lines[1]).toBe(first);

    // ...a redraw replaces it, so a renderer sees that it changed...
    proc.process([chunk('\r20%')]);
    const second = proc.lines[1];
    expect(second).not.toBe(first);
    expect(second).toEqual({ id: first.id, text: '20%', stream: 'stdout' });

    // ...and a line that ends as it stood stays the object that was on show.
    proc.process([chunk('\nnext')]);
    expect(proc.lines[1]).toBe(second);
    expect(proc.lines.map((l) => l.text)).toEqual(['done', '20%', 'next']);
  });

  it('numbers lines in order and keeps a line its number once it is finished', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('a\nb\nc')]);
    expect(proc.lines.map((l) => l.id)).toEqual([0, 1, 2]);

    proc.process([chunk('!\rC\nd\n')]);
    expect(proc.lines.map((l) => [l.id, l.text])).toEqual([
      [0, 'a'],
      [1, 'b'],
      [2, 'C'],
      [3, 'd'],
    ]);
  });

  it('keeps a line on show while a trailing \\r is undecided', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('a\nwork')]);
    proc.process([chunk('\r')]);
    // Still on show: the \r may yet be half of a \r\n.
    expect(proc.lines.map((l) => l.text)).toEqual(['a', 'work']);
    proc.process([chunk('\n')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['a', 'work']);
    expect(proc.lines.map((l) => l.id)).toEqual([0, 1]);
  });

  it('takes the unfinished line away once an overwrite has emptied it', () => {
    const proc = createIncrementalProcessor();
    proc.process([chunk('a\nwork\r')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['a', 'work']);
    // A second \r settles the first as an overwrite, with nothing written yet.
    proc.process([chunk('\r')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['a']);
    proc.process([chunk('again')]);
    expect(proc.lines.map((l) => [l.id, l.text])).toEqual([
      [0, 'a'],
      [1, 'again'],
    ]);
  });

  // ---------------------------------------------------------------------------
  // Limits
  // ---------------------------------------------------------------------------

  it('keeps only the most recent lines and counts the rest', () => {
    const proc = createIncrementalProcessor(limits({ maxLines: 3 }));
    for (let n = 0; n < 10; n++) {
      proc.process([chunk(`line ${n}\n`)]);
      expect(proc.lines.length).toBeLessThanOrEqual(3);
    }

    expect(proc.lines.map((l) => l.text)).toEqual(['line 7', 'line 8', 'line 9']);
    // Lines keep the numbers they had, so a keyed list sees three removals
    // and three additions, not ten changed rows.
    expect(proc.lines.map((l) => l.id)).toEqual([7, 8, 9]);
    expect(proc.droppedLines).toBe(7);
  });

  it('counts the unfinished line towards the limit', () => {
    const proc = createIncrementalProcessor(limits({ maxLines: 3 }));
    proc.process([chunk('a\nb\nc\nd')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['b', 'c', 'd']);
    expect(proc.droppedLines).toBe(1);

    // A redraw of the unfinished line drops nothing more.
    proc.process([chunk('\rD')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['b', 'c', 'D']);
    expect(proc.droppedLines).toBe(1);
  });

  it('trims a large snapshot as it goes, to the same result', () => {
    const proc = createIncrementalProcessor(limits({ maxLines: 100 }));
    const snapshot = Array.from({ length: 12_345 }, (_, n) => `line ${n}\n`).join('');
    proc.process([chunk(snapshot.slice(0, 40_000)), chunk(snapshot.slice(40_000))]);

    expect(proc.lines.length).toBe(100);
    expect(proc.lines[0]).toMatchObject({ id: 12_245, text: 'line 12245' });
    expect(proc.lines[99]).toMatchObject({ id: 12_344, text: 'line 12344' });
    expect(proc.droppedLines).toBe(12_245);
  });

  it('keeps the total text within the character limit', () => {
    const proc = createIncrementalProcessor(limits({ maxChars: 25 }));
    proc.process([chunk('aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc\n')]);
    // Thirty characters of lines: the oldest goes.
    expect(proc.lines.map((l) => l.text)).toEqual(['bbbbbbbbbb', 'cccccccccc']);
    expect(proc.droppedLines).toBe(1);

    // The unfinished line counts too.
    proc.process([chunk('dddddddd')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['cccccccccc', 'dddddddd']);
    expect(proc.droppedLines).toBe(2);
  });

  it('cuts a line that grows past the line limit, and says so', () => {
    const proc = createIncrementalProcessor(limits({ maxLineChars: 10 }));
    proc.process([chunk('0123456')]);
    proc.process([chunk('789abcdef')]);
    proc.process([chunk('more of the same line')]);
    expect(proc.lines.map((l) => l.text)).toEqual(['0123456789' + LINE_CUT_MARK]);

    // The next line starts afresh, and so does a redraw of the cut one.
    proc.process([chunk('\nshort\n0123456789abc\rredrawn')]);
    expect(proc.lines.map((l) => l.text)).toEqual([
      '0123456789' + LINE_CUT_MARK,
      'short',
      'redrawn',
    ]);
  });

  it('does not cut a line in the middle of a surrogate pair', () => {
    const proc = createIncrementalProcessor(limits({ maxLineChars: 4 }));
    proc.process([chunk('abc😀def\n')]);
    expect(proc.lines[0].text).toBe('abc' + LINE_CUT_MARK);
  });

  it('holds a bounded amount however long the output runs', () => {
    const proc = createIncrementalProcessor();
    const batch = Array.from({ length: 500 }, (_, n) => `${'x'.repeat(100)} ${n}\n`).join('');
    for (let n = 0; n < 40; n++) proc.process([chunk(batch)]);
    // One line with no break at all, far longer than a line may be.
    for (let n = 0; n < 40; n++) proc.process([chunk('y'.repeat(10_000))]);

    expect(proc.lines.length).toBe(OUTPUT_LINE_LIMITS.maxLines);
    expect(proc.droppedLines).toBe(40 * 500 + 1 - OUTPUT_LINE_LIMITS.maxLines);
    const last = proc.lines[proc.lines.length - 1];
    expect(last.text.length).toBe(OUTPUT_LINE_LIMITS.maxLineChars + LINE_CUT_MARK.length);
  });
});
