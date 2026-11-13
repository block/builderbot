import { describe, expect, it, vi } from 'vitest';
import type { BufferedOutput, OutputChunk } from './actions';
import {
  MAX_HELD_CHARS,
  createOutputFollower,
  utf8Length,
  type OutputEvent,
  type OutputFollower,
} from './outputFollower';

/** Let pending promise callbacks run. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

function stdout(text: string): OutputChunk {
  return { chunk: text, stream: 'stdout', timestamp: 0 };
}

/**
 * A stand-in for the backend's buffer and event stream, with the properties
 * the follower relies on: output is appended to the buffer before it is
 * emitted, every event says where its chunk starts, and a snapshot says where
 * it ends.
 */
function backend() {
  let text = '';
  let bytes = 0;
  return {
    get text() {
      return text;
    },
    /** Append `chunk` to the buffer and return the event that announces it. */
    write(chunk: string): OutputEvent {
      const event: OutputEvent = { chunk, stream: 'stdout', offset: bytes };
      text += chunk;
      bytes += utf8Length(chunk);
      return event;
    },
    snapshot(): BufferedOutput {
      return { chunks: text ? [stdout(text)] : [], endOffset: bytes };
    },
  };
}

/** A follower whose snapshots land when the test says, and what it has shown. */
function harness() {
  const pending: {
    resolve: (snapshot: BufferedOutput | null) => void;
    reject: (error: unknown) => void;
  }[] = [];
  const view = {
    text: '',
    /** How many snapshots replaced the view. */
    snapshots: 0,
    /** How many live chunks were appended. */
    appended: 0,
    unavailable: 0,
    errors: [] as unknown[],
  };
  const follower: OutputFollower = createOutputFollower({
    fetchSnapshot: () => new Promise((resolve, reject) => void pending.push({ resolve, reject })),
    onSnapshot: (chunks) => {
      view.text = chunks.map((c) => c.chunk).join('');
      view.snapshots++;
    },
    onOutput: (output) => {
      view.text += output.chunk;
      view.appended++;
    },
    onUnavailable: () => void view.unavailable++,
    onError: (error) => void view.errors.push(error),
  });
  return {
    follower,
    view,
    /** Snapshots requested and not yet landed. */
    get inFlight() {
      return pending.length;
    },
    /** Land the oldest snapshot in flight. */
    async land(snapshot: BufferedOutput | null) {
      pending.shift()!.resolve(snapshot);
      await settle();
    },
    async fail(error: unknown) {
      pending.shift()!.reject(error);
      await settle();
    },
  };
}

describe('utf8Length', () => {
  it('counts bytes the way the backend does', () => {
    const encoder = new TextEncoder();
    for (const text of [
      '',
      'plain ascii\r\n',
      'café',
      '€ three bytes',
      '😀 four bytes, two units',
      'mixed: aé€😀\u{10FFFF}߿ࠀ￿',
      '\x1b[32m✓\x1b[0m done',
    ]) {
      expect(utf8Length(text), JSON.stringify(text)).toBe(encoder.encode(text).length);
    }
  });

  it('counts an unpaired surrogate as the replacement character it encodes to', () => {
    expect(utf8Length('a\ud800b')).toBe(new TextEncoder().encode('a\ud800b').length);
    expect(utf8Length('\udc00')).toBe(3);
    expect(utf8Length('\ud83d')).toBe(3);
  });
});

describe('createOutputFollower', () => {
  it('shows the snapshot, then the events that continue it', async () => {
    const be = backend();
    const { follower, view, land } = harness();
    be.write('one\n');

    follower.sync();
    await land(be.snapshot());
    expect(view.text).toBe('one\n');

    follower.handleEvent(be.write('two\n'));
    follower.handleEvent(be.write('thrée\n'));
    follower.handleEvent(be.write('four\n'));
    expect(view.text).toBe('one\ntwo\nthrée\nfour\n');
    expect(view).toMatchObject({ snapshots: 1, appended: 3 });
  });

  it('skips events the snapshot already holds, whichever side of it they arrive on', async () => {
    const be = backend();
    const { follower, view, land } = harness();
    be.write('before the listener\n');

    follower.sync();
    // Both of these are written before the snapshot is taken, so it holds
    // them. One is delivered ahead of it...
    const early = be.write('early\n');
    follower.handleEvent(early);
    // ...and the other only after it has landed.
    const late = be.write('late\n');
    const snapshot = be.snapshot();

    // Written after the snapshot was taken, delivered before it lands.
    const after = be.write('after\n');
    follower.handleEvent(after);

    await land(snapshot);
    expect(view.text).toBe('before the listener\nearly\nlate\nafter\n');

    follower.handleEvent(late);
    follower.handleEvent(be.write('live\n'));
    expect(view.text).toBe(be.text);
    expect(view).toMatchObject({ snapshots: 1, appended: 2 });
  });

  it('does not repeat or lose identical chunks at the boundary', async () => {
    // A progress bar redraws the same text over and over, so the text alone
    // cannot say which redraws the snapshot holds. The offsets do.
    const be = backend();
    const { follower, view, land } = harness();
    const frame = '\rdownloading 50%';

    follower.sync();
    const first = be.write(frame);
    const second = be.write(frame);
    const snapshot = be.snapshot();
    const third = be.write(frame);
    const fourth = be.write(frame);
    for (const event of [first, second, third]) follower.handleEvent(event);

    await land(snapshot);
    follower.handleEvent(fourth);

    expect(view.text).toBe(frame.repeat(4));
    expect(view).toMatchObject({ snapshots: 1, appended: 2 });
  });

  it('ignores events until it is asked to sync', async () => {
    const be = backend();
    const { follower, view, land, inFlight } = harness();

    follower.handleEvent(be.write('too early\n'));
    expect(view.text).toBe('');
    expect(inFlight).toBe(0);

    follower.sync();
    await land(be.snapshot());
    expect(view.text).toBe('too early\n');
  });

  it('takes a new snapshot when an event does not continue what is shown', async () => {
    const be = backend();
    const h = harness();
    h.follower.sync();
    await h.land(be.snapshot());
    h.follower.handleEvent(be.write('one\n'));

    // Two events never arrive.
    be.write('lost\n');
    be.write('also lost\n');
    h.follower.handleEvent(be.write('four\n'));
    // Nothing is shown out of place meanwhile.
    expect(h.view.text).toBe('one\n');
    expect(h.inFlight).toBe(1);

    // More arrives while the snapshot is on its way.
    const snapshot = be.snapshot();
    h.follower.handleEvent(be.write('five\n'));
    await h.land(snapshot);

    expect(h.view.text).toBe('one\nlost\nalso lost\nfour\nfive\n');
    expect(h.view).toMatchObject({ snapshots: 2, appended: 2 });

    h.follower.handleEvent(be.write('six\n'));
    expect(h.view.text).toBe(be.text);
    expect(h.inFlight).toBe(0);
  });

  it('takes a new snapshot when an event overlaps the end of what is shown', async () => {
    const h = harness();
    h.follower.sync();
    await h.land({ chunks: [stdout('abcd')], endOffset: 4 });

    // The backend never emits this; whatever did, the view must not guess.
    h.follower.handleEvent({ chunk: 'cdef', stream: 'stdout', offset: 2 });
    expect(h.view.text).toBe('abcd');
    expect(h.inFlight).toBe(1);

    await h.land({ chunks: [stdout('abcdef')], endOffset: 6 });
    expect(h.view.text).toBe('abcdef');
  });

  it('retakes a snapshot that a second sync may have outdated', async () => {
    const be = backend();
    const h = harness();
    be.write('one\n');

    h.follower.sync();
    const stale = be.snapshot();
    // The stream reconnects: output written while it was down is in no event.
    be.write('written while disconnected\n');
    h.follower.sync();
    expect(h.inFlight).toBe(1);

    await h.land(stale);
    // The first snapshot is not shown; a second is on its way instead.
    expect(h.view.snapshots).toBe(0);
    expect(h.inFlight).toBe(1);

    await h.land(be.snapshot());
    expect(h.view.text).toBe('one\nwritten while disconnected\n');
    expect(h.view.snapshots).toBe(1);
  });

  it('leaves the view alone when a later snapshot holds nothing new', async () => {
    const be = backend();
    const h = harness();
    h.follower.sync();
    await h.land(be.snapshot());
    h.follower.handleEvent(be.write('one\n'));
    h.follower.handleEvent(be.write('two\n'));

    // A reconnect during which nothing was written.
    h.follower.sync();
    await h.land(be.snapshot());

    expect(h.view).toMatchObject({ text: 'one\ntwo\n', snapshots: 1, appended: 2 });
    h.follower.handleEvent(be.write('three\n'));
    expect(h.view.text).toBe(be.text);
  });

  it('reports output that is no longer available, once, and stops following', async () => {
    const h = harness();
    h.follower.sync();
    h.follower.handleEvent({ chunk: 'stray', stream: 'stdout', offset: 0 });
    await h.land(null);

    expect(h.view).toMatchObject({ text: '', unavailable: 1, snapshots: 0 });

    h.follower.handleEvent({ chunk: 'stray', stream: 'stdout', offset: 5 });
    h.follower.sync();
    expect(h.inFlight).toBe(0);
    expect(h.view.text).toBe('');
  });

  it('keeps what is shown when the output is cleared afterwards', async () => {
    const be = backend();
    const h = harness();
    be.write('all of it\n');
    h.follower.sync();
    await h.land(be.snapshot());

    // A reconnect after the finished execution's buffer was cleared.
    h.follower.sync();
    await h.land(null);

    expect(h.view).toMatchObject({ text: 'all of it\n', unavailable: 0 });
  });

  it('reports a snapshot that fails, and recovers on the next sync', async () => {
    const be = backend();
    const h = harness();
    be.write('one\n');

    h.follower.sync();
    h.follower.handleEvent(be.write('two\n'));
    await h.fail(new Error('IPC unavailable'));
    expect(h.view.errors).toHaveLength(1);
    expect(h.view.text).toBe('');

    // Events have nothing to continue until a snapshot is shown.
    h.follower.handleEvent(be.write('three\n'));
    expect(h.view.text).toBe('');
    expect(h.inFlight).toBe(0);

    h.follower.sync();
    await h.land(be.snapshot());
    expect(h.view.text).toBe('one\ntwo\nthree\n');
  });

  it('retries after a failed resync when the next event does not fit', async () => {
    const be = backend();
    const h = harness();
    h.follower.sync();
    await h.land(be.snapshot());
    h.follower.handleEvent(be.write('one\n'));

    be.write('lost\n');
    h.follower.handleEvent(be.write('three\n'));
    await h.fail(new Error('IPC unavailable'));
    expect(h.view).toMatchObject({ text: 'one\n' });
    expect(h.view.errors).toHaveLength(1);

    h.follower.handleEvent(be.write('four\n'));
    await h.land(be.snapshot());
    expect(h.view.text).toBe('one\nlost\nthree\nfour\n');
  });

  it('gives up on held events past the limit and takes a newer snapshot', async () => {
    const be = backend();
    const h = harness();
    h.follower.sync();
    const stale = be.snapshot();

    const big = 'x'.repeat(MAX_HELD_CHARS / 4);
    for (let n = 0; n < 5; n++) h.follower.handleEvent(be.write(big));
    h.follower.handleEvent(be.write('tail\n'));

    await h.land(stale);
    // The dropped events followed that snapshot, so it is not shown.
    expect(h.view.snapshots).toBe(0);
    expect(h.inFlight).toBe(1);

    await h.land(be.snapshot());
    expect(h.view.text.length).toBe(be.text.length);
    expect(h.view.text.endsWith('tail\n')).toBe(true);
    expect(h.view).toMatchObject({ snapshots: 1, appended: 0 });
  });

  it('does nothing once stopped', async () => {
    const be = backend();
    const h = harness();
    h.follower.sync();
    await h.land(be.snapshot());
    h.follower.handleEvent(be.write('one\n'));

    h.follower.sync();
    h.follower.stop();
    be.write('two\n');
    await h.land(be.snapshot());
    h.follower.handleEvent(be.write('three\n'));
    h.follower.sync();

    expect(h.view).toMatchObject({ text: 'one\n', snapshots: 1, appended: 1 });
    expect(h.inFlight).toBe(0);
  });

  it('shows exactly the output written, whatever order things arrive in', async () => {
    // A fixed pseudo-random sequence, so a failure reproduces.
    let seed = 0x51ed27;
    const random = (below: number) => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed % below;
    };
    const pieces = ['.', '\rframe', 'line\n', 'é', '€uro\n', '😀', '\r\n', 'same', 'same'];

    for (let round = 0; round < 400; round++) {
      const be = backend();
      const fetchSnapshot = vi.fn<() => Promise<BufferedOutput | null>>();
      let shown = '';
      const follower = createOutputFollower({
        fetchSnapshot,
        onSnapshot: (chunks) => void (shown = chunks.map((c) => c.chunk).join('')),
        onOutput: (output) => void (shown += output.chunk),
        onUnavailable: () => {
          throw new Error('unexpected: unavailable');
        },
        onError: (error) => {
          throw error;
        },
      });
      // Snapshots are taken when requested and land some steps later.
      const landing: { at: number; land: () => void }[] = [];
      let step = 0;
      fetchSnapshot.mockImplementation(() => {
        const snapshot = be.snapshot();
        return new Promise((resolve) => {
          landing.push({ at: step + random(4), land: () => resolve(snapshot) });
        });
      });

      /** Events emitted but not yet delivered, in order. */
      let inTransit: OutputEvent[] = [];
      let listening = false;
      const steps = 20 + random(40);
      const listenAt = random(10);
      for (step = 0; step < steps; step++) {
        // The backend writes; what it writes before the listener is live
        // reaches nobody.
        for (let n = random(3); n > 0; n--) {
          const event = be.write(pieces[random(pieces.length)]);
          if (listening) inTransit.push(event);
        }
        if (!listening && step >= listenAt) {
          listening = true;
          follower.sync();
        }
        // Now and then the stream drops what is in transit, and says so.
        if (listening && random(12) === 0) {
          inTransit = [];
          follower.sync();
        }
        // Some of what is in transit is delivered, in order.
        for (let n = random(inTransit.length + 1); n > 0; n--) {
          follower.handleEvent(inTransit.shift()!);
        }
        for (const due of landing.filter((l) => l.at <= step)) {
          landing.splice(landing.indexOf(due), 1);
          due.land();
        }
        await settle();
      }
      // Everything in flight arrives.
      for (let guard = 0; landing.length > 0 || inTransit.length > 0; guard++) {
        if (guard > 100) throw new Error('did not settle');
        for (const event of inTransit.splice(0)) follower.handleEvent(event);
        for (const due of landing.splice(0)) due.land();
        await settle();
      }

      expect(shown, `round ${round}`).toBe(be.text);
    }
  });
});
