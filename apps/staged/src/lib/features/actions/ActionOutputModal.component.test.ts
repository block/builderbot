import { flushSync, mount, tick, unmount } from 'svelte';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ListenOptions } from '$lib/transport';
import type { ActionOutputEvent, ActionStatusEvent, BufferedOutput } from './actions';

// The modal under the real dialog primitive, with the backend replaced by a
// fake the tests drive: a buffer snapshot that lands when the test says, and
// output and status listeners the test can fire. What is covered here is the
// wiring around outputFollower and processOutput, which have their own
// tests: the order the modal subscribes and fetches in, what it renders at
// the join, how much of the list a live update touches, and what it lets go
// of when it closes.

const backend = vi.hoisted(() => {
  interface Listener<T> {
    callback: (event: T) => void;
    opts?: ListenOptions;
    live: boolean;
  }
  interface Fetch {
    executionId: string;
    land: (snapshot: BufferedOutput | null) => void;
    fail: (error: unknown) => void;
  }
  return {
    /** Every backend call the modal made, in order. */
    calls: [] as string[],
    outputListeners: [] as Listener<ActionOutputEvent>[],
    statusListeners: [] as Listener<ActionStatusEvent>[],
    fetches: [] as Fetch[],
    sanitized: [] as string[],
    processors: [] as { lines: unknown[] }[],
    createNote: vi.fn(async () => {}),
    clearActionExecution: vi.fn(async () => true),
    stopBranchAction: vi.fn(async () => {}),
  };
});

vi.mock('./actions', () => {
  function listen<T>(
    name: string,
    listeners: { callback: (event: T) => void; opts?: ListenOptions; live: boolean }[]
  ) {
    return (callback: (event: T) => void, opts?: ListenOptions) => {
      backend.calls.push(`listen:${name}`);
      const listener = { callback, opts, live: true };
      listeners.push(listener);
      return () => {
        backend.calls.push(`unlisten:${name}`);
        listener.live = false;
      };
    };
  }
  return {
    getActionOutputBuffer: (executionId: string) => {
      backend.calls.push(`fetch:${executionId}`);
      return new Promise<BufferedOutput | null>((land, fail) => {
        backend.fetches.push({ executionId, land, fail });
      });
    },
    listenToActionOutput: listen('output', backend.outputListeners),
    listenToActionStatus: listen('status', backend.statusListeners),
    stopBranchAction: backend.stopBranchAction,
    clearActionExecution: backend.clearActionExecution,
  };
});
vi.mock('../../commands', () => ({ createNote: backend.createNote }));
// The real sanitizer and line processor, with a record of what each was
// asked for: the lines rendered to HTML, and the processors made to hold
// lines.
vi.mock('../../shared/sanitize', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../shared/sanitize')>();
  return {
    sanitize: (dirty: string) => {
      backend.sanitized.push(dirty);
      return actual.sanitize(dirty);
    },
  };
});
vi.mock('./processOutput', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./processOutput')>();
  return {
    ...actual,
    createIncrementalProcessor: (...args: Parameters<typeof actual.createIncrementalProcessor>) => {
      const processor = actual.createIncrementalProcessor(...args);
      backend.processors.push(processor);
      return processor;
    },
  };
});

import ActionOutputModal from './ActionOutputModal.svelte';
import { OUTPUT_LINE_LIMITS } from './processOutput';
import { reactiveProps } from '../sessions/testStubs/reactiveProps.svelte';

const encoder = new TextEncoder();

/** A fake execution's output, as the backend's buffer and events present it. */
function execution(executionId: string) {
  const chunks: { chunk: string; stream: 'stdout' | 'stderr'; offset: number }[] = [];
  let end = 0;
  return {
    /** Write `chunk`. Returns the event the backend emits for it. */
    write(chunk: string, stream: 'stdout' | 'stderr' = 'stdout'): ActionOutputEvent {
      const event = { executionId, chunk, stream, offset: end };
      chunks.push({ chunk, stream, offset: end });
      end += encoder.encode(chunk).length;
      return event;
    },
    /** The buffer as it now stands. */
    snapshot(): BufferedOutput {
      return {
        chunks: chunks.map(({ chunk, stream }) => ({ chunk, stream, timestamp: 0 })),
        endOffset: end,
      };
    },
  };
}

type Props = {
  open: boolean;
  executionId: string;
  branchId?: string;
  actionName: string;
  onClose: () => void;
};

let component: ReturnType<typeof mount> | null = null;
let props: Props;

/** Let promise callbacks, the dialog's frames and the re-render all land. */
async function settle() {
  // The dialog mounts and unmounts, and the modal publishes live output, on
  // animation frames.
  await new Promise((resolve) => requestAnimationFrame(resolve));
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    await tick();
    flushSync();
  }
}

async function mountModal(initial: Partial<Props> = {}) {
  props = reactiveProps<Props>({
    open: true,
    executionId: 'execution-1',
    branchId: 'branch-1',
    actionName: 'Test',
    onClose: vi.fn(),
    ...initial,
  });
  const target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(ActionOutputModal, { target, props });
  await settle();
}

const liveOutputListeners = () => backend.outputListeners.filter((l) => l.live);
const liveStatusListeners = () => backend.statusListeners.filter((l) => l.live);

/** Tell the modal its output listener is live (or live again). */
function establish() {
  for (const listener of liveOutputListeners()) listener.opts?.onEstablished?.();
}

function emit(event: ActionOutputEvent) {
  for (const listener of liveOutputListeners()) listener.callback(event);
}

/** Land the oldest snapshot still in flight. */
async function land(snapshot: BufferedOutput | null) {
  const fetch = backend.fetches.shift();
  if (!fetch) throw new Error('no snapshot is in flight');
  fetch.land(snapshot);
  await settle();
}

const rows = () => [...document.querySelectorAll<HTMLElement>('.output-line:not(.dropped-lines)')];
const rowTexts = () => rows().map((row) => row.textContent?.trim());
const centerState = () => document.querySelector('.center-state')?.textContent?.trim() ?? null;
const droppedNotice = () => document.querySelector('.dropped-lines')?.textContent?.trim() ?? null;

beforeAll(() => {
  // jsdom lacks the layout APIs the dialog's viewport tracking uses.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
});

beforeEach(() => {
  backend.calls.length = 0;
  backend.outputListeners.length = 0;
  backend.statusListeners.length = 0;
  backend.fetches.length = 0;
  backend.sanitized.length = 0;
  backend.processors.length = 0;
  backend.createNote.mockClear();
  vi.spyOn(console, 'error').mockImplementation(() => {});
});

afterEach(async () => {
  if (component) unmount(component);
  component = null;
  await settle();
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});

describe('opening', () => {
  it('listens for output before it reads the buffer', async () => {
    await mountModal();

    // Nothing is read until the listener says it is live: output written in
    // between would be in neither the snapshot nor an event.
    expect(backend.calls).toEqual(['listen:output', 'listen:status']);
    expect(centerState()).toBe('Loading output…');

    establish();
    expect(backend.calls).toEqual(['listen:output', 'listen:status', 'fetch:execution-1']);
  });

  it('still shows the buffer when the listener cannot be registered', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();

    liveOutputListeners()[0].opts?.onRegistrationFailed?.(new Error('no event channel'));
    await land(run.snapshot());

    expect(rowTexts()).toEqual(['one']);
  });

  it('shows each chunk once when events land on both sides of the snapshot', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();
    establish();

    // Written while the snapshot is in flight. The snapshot is taken after
    // the first of these and before the second, and both events reach the
    // modal ahead of it.
    const inSnapshot = run.write('two\n');
    const snapshot = run.snapshot();
    const afterSnapshot = run.write('three\n');
    emit(inSnapshot);
    emit(afterSnapshot);
    await land(snapshot);
    expect(rowTexts()).toEqual(['one', 'two', 'three']);

    // An event the snapshot already held, delivered late.
    emit(inSnapshot);
    emit(run.write('four\n'));
    await settle();
    expect(rowTexts()).toEqual(['one', 'two', 'three', 'four']);
  });

  it('joins a progress bar that repeats itself without dropping or doubling a tick', async () => {
    // Identical chunks: nothing in their text says which the snapshot holds.
    const run = execution('execution-1');
    const ticks = [run.write('.'), run.write('.'), run.write('.')];
    await mountModal();
    establish();
    const snapshot = run.snapshot();
    ticks.push(run.write('.'), run.write('.'));
    for (const tick of ticks) emit(tick);
    await land(snapshot);

    expect(rowTexts()).toEqual(['.....']);
  });

  it('ignores the output of other executions', async () => {
    const run = execution('execution-1');
    const other = execution('execution-2');
    await mountModal();
    establish();
    await land(run.snapshot());

    emit(other.write('not mine\n'));
    emit(run.write('mine\n'));
    await settle();

    expect(rowTexts()).toEqual(['mine']);
  });

  it('says so when a running action has written nothing yet', async () => {
    await mountModal();
    establish();
    await land(execution('execution-1').snapshot());

    expect(centerState()).toBe('No output yet…');
  });

  it('says so when the output has been cleared or evicted', async () => {
    await mountModal();
    establish();
    await land(null);

    expect(centerState()).toBe('Output is no longer available.');
    expect(rows()).toHaveLength(0);
  });

  it('shows the failure when the buffer cannot be read', async () => {
    await mountModal();
    establish();
    backend.fetches.shift()!.fail(new Error('IPC unavailable'));
    await settle();

    expect(centerState()).toBe('IPC unavailable');
  });

  it('reads nothing and listens to nothing while closed', async () => {
    await mountModal({ open: false, executionId: '' });

    expect(backend.calls).toEqual([]);
    expect(document.querySelector('.output-line')).toBeNull();
  });
});

describe('following live output', () => {
  it('renders only the lines that changed', async () => {
    const run = execution('execution-1');
    run.write('one\ntwo\nthr');
    await mountModal();
    establish();
    await land(run.snapshot());
    expect(rowTexts()).toEqual(['one', 'two', 'thr']);
    expect(backend.sanitized).toEqual(['one', 'two', 'thr']);
    const before = rows();

    backend.sanitized.length = 0;
    emit(run.write('ee\nfour\n'));
    await settle();

    expect(rowTexts()).toEqual(['one', 'two', 'three', 'four']);
    // The finished lines were neither rendered to HTML again nor replaced in
    // the document; the unfinished one was updated where it stood.
    expect([...backend.sanitized].sort()).toEqual(['four', 'three']);
    const after = rows();
    expect(after.slice(0, 3).every((row, i) => row === before[i])).toBe(true);
  });

  it('does not render a line again when it finishes as it stood', async () => {
    const run = execution('execution-1');
    run.write('building');
    await mountModal();
    establish();
    await land(run.snapshot());
    backend.sanitized.length = 0;

    emit(run.write('\n'));
    emit(run.write('done'));
    await settle();

    expect(rowTexts()).toEqual(['building', 'done']);
    expect(backend.sanitized).toEqual(['done']);
  });

  it('keeps one processor and one line array for the whole of a run', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();
    establish();
    await land(run.snapshot());
    const processor = backend.processors.at(-1)!;
    const lines = processor.lines;

    for (let i = 0; i < 50; i++) {
      emit(run.write(`line ${i}\n`));
      if (i % 10 === 0) await settle();
    }
    await settle();

    expect(rows()).toHaveLength(51);
    // Live output is added to the array the snapshot filled: no update
    // copies the lines or starts a new holder for them.
    expect(backend.processors.at(-1)).toBe(processor);
    expect(processor.lines).toBe(lines);
    expect(lines).toHaveLength(51);
  });

  it('overwrites a line on a carriage return', async () => {
    const run = execution('execution-1');
    run.write('10%');
    await mountModal();
    establish();
    await land(run.snapshot());

    emit(run.write('\r50%'));
    emit(run.write('\r100%\n'));
    await settle();

    expect(rowTexts()).toEqual(['100%']);
  });

  it('marks stderr lines', async () => {
    const run = execution('execution-1');
    run.write('out\n');
    run.write('err\n', 'stderr');
    await mountModal();
    establish();
    await land(run.snapshot());

    expect(rows().map((row) => row.classList.contains('stderr'))).toEqual([false, true]);
  });

  it('keeps the output inert', async () => {
    const run = execution('execution-1');
    run.write('<img src=x onerror=alert(1)> \u001b[31mred\u001b[0m\n');
    await mountModal();
    establish();
    await land(run.snapshot());

    expect(rows()[0].querySelector('img')).toBeNull();
    expect(rowTexts()).toEqual(['<img src=x onerror=alert(1)> red']);
  });

  it('reads the buffer again when the listener is re-established', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();
    establish();
    await land(run.snapshot());

    // A reconnect: these were written while nothing was listening.
    run.write('two\n');
    run.write('three\n');
    establish();
    expect(backend.fetches).toHaveLength(1);
    await land(run.snapshot());

    expect(rowTexts()).toEqual(['one', 'two', 'three']);
  });

  it('reads the buffer again when an event shows that one was lost', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();
    establish();
    await land(run.snapshot());

    run.write('two\n');
    emit(run.write('three\n'));
    await settle();
    // Not shown with a hole in it.
    expect(rowTexts()).toEqual(['one']);
    await land(run.snapshot());

    expect(rowTexts()).toEqual(['one', 'two', 'three']);
  });

  it('keeps what is on show when a later read of the buffer fails', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();
    establish();
    await land(run.snapshot());

    establish();
    backend.fetches.shift()!.fail(new Error('IPC unavailable'));
    await settle();

    expect(rowTexts()).toEqual(['one']);
    expect(centerState()).toBeNull();
  });
});

describe('the line limit', () => {
  it('shows the most recent lines and counts the rest', async () => {
    const run = execution('execution-1');
    const total = OUTPUT_LINE_LIMITS.maxLines + 3;
    let text = '';
    for (let i = 0; i < total; i++) text += `line ${i}\n`;
    run.write(text);
    await mountModal();
    establish();
    await land(run.snapshot());

    expect(rows()).toHaveLength(OUTPUT_LINE_LIMITS.maxLines);
    expect(rows()[0].textContent?.trim()).toBe('line 3');
    expect(rows().at(-1)?.textContent?.trim()).toBe(`line ${total - 1}`);
    expect(droppedNotice()).toBe('3 earlier lines not shown');
    // The lines that were dropped were never rendered to HTML.
    expect(backend.sanitized).toHaveLength(OUTPUT_LINE_LIMITS.maxLines);

    backend.sanitized.length = 0;
    emit(run.write('one more\n'));
    await settle();
    // Dropping a line from the front renders none of the others again.
    expect(backend.sanitized).toEqual(['one more']);
    expect(rows()).toHaveLength(OUTPUT_LINE_LIMITS.maxLines);
    expect(rows()[0].textContent?.trim()).toBe('line 4');
    expect(rows().at(-1)?.textContent?.trim()).toBe('one more');
    expect(droppedNotice()).toBe('4 earlier lines not shown');
  });

  it('shows no count while nothing has been dropped', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();
    establish();
    await land(run.snapshot());

    expect(droppedNotice()).toBeNull();
  });
});

describe('closing', () => {
  it('stops listening and lets go of the lines once the dialog has gone', async () => {
    const run = execution('execution-1');
    run.write('one\ntwo\n');
    await mountModal();
    establish();
    await land(run.snapshot());
    const held = backend.processors.at(-1)!;
    expect(held.lines).toHaveLength(2);

    props.open = false;
    await settle();
    await vi.waitFor(() => expect(document.querySelector('.modal-content')).toBeNull());
    await settle();

    expect(liveOutputListeners()).toHaveLength(0);
    expect(liveStatusListeners()).toHaveLength(0);
    // The component, still mounted as it is on a branch card, has swapped
    // the processor that held the lines for an empty one.
    const now = backend.processors.at(-1)!;
    expect(now).not.toBe(held);
    expect(now.lines).toHaveLength(0);
  });

  it('shows nothing of the last execution when it is opened on another', async () => {
    const first = execution('execution-1');
    first.write('from the first\n');
    await mountModal();
    establish();
    await land(first.snapshot());

    props.open = false;
    await settle();
    await vi.waitFor(() => expect(document.querySelector('.modal-content')).toBeNull());
    props.executionId = 'execution-2';
    props.open = true;
    await settle();

    expect(centerState()).toBe('Loading output…');
    expect(rows()).toHaveLength(0);
    const second = execution('execution-2');
    second.write('from the second\n');
    establish();
    expect(backend.fetches.map((f) => f.executionId)).toEqual(['execution-2']);
    await land(second.snapshot());
    expect(rowTexts()).toEqual(['from the second']);
  });

  it('ignores a snapshot and events that arrive after it closed', async () => {
    const run = execution('execution-1');
    run.write('one\n');
    await mountModal();
    establish();
    const stale = backend.fetches.shift()!;
    const listener = liveOutputListeners()[0];

    props.open = false;
    await settle();
    stale.land(run.snapshot());
    listener.callback(run.write('two\n'));
    await settle();

    expect(backend.processors.every((processor) => processor.lines.length === 0)).toBe(true);
  });

  it('follows the new execution when Run again swaps it while open', async () => {
    const first = execution('execution-1');
    first.write('first run\n');
    await mountModal();
    establish();
    await land(first.snapshot());

    props.executionId = 'execution-2';
    await settle();
    expect(liveOutputListeners()).toHaveLength(1);
    expect(centerState()).toBe('Loading output…');

    const second = execution('execution-2');
    establish();
    await land(second.snapshot());
    emit(first.write('late, from the first\n'));
    emit(second.write('second run\n'));
    await settle();

    expect(rowTexts()).toEqual(['second run']);
  });

  it('saves the lines it holds as a note', async () => {
    const run = execution('execution-1');
    run.write('one\ntwo\n');
    await mountModal();
    establish();
    await land(run.snapshot());

    const save = [...document.querySelectorAll('button')].find((button) =>
      button.textContent?.includes('Save log')
    )!;
    save.click();
    await settle();

    expect(backend.createNote).toHaveBeenCalledWith('branch-1', 'Test log', '```\none\ntwo\n```');
  });
});
