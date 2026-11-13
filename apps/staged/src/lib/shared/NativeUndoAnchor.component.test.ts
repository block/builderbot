import { flushSync, mount, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import NativeUndoAnchor from './NativeUndoAnchor.svelte';
import { installNativeUndoStack, type NativeUndoStack } from './testStubs/nativeUndoStack';

// The anchor's side of the browser's undo stack: what `record` puts on it,
// and which steps run as undo and redo walk it. jsdom has no such stack, so
// one is stood in (testStubs/nativeUndoStack.ts); that a real browser keeps
// the anchor's edits in order with those of the inputs around it is what
// the component is built on, and isn't something jsdom can show.

let component: ReturnType<typeof mount<Record<string, never>, { record: Recorder }>> | null = null;
let target: HTMLElement;
let stack: NativeUndoStack;

type Recorder = (step: { undo: () => void; redo: () => void }) => boolean;

function render() {
  target = document.createElement('div');
  document.body.appendChild(target);
  component = mount(NativeUndoAnchor, { target });
  flushSync();
}

function record(name: string, log: string[]): boolean {
  return component!.record({
    undo: () => log.push(`undo ${name}`),
    redo: () => log.push(`redo ${name}`),
  });
}

function anchor(): HTMLElement {
  const el = target.querySelector<HTMLElement>('.native-undo-anchor');
  if (!el) throw new Error('no anchor');
  return el;
}

/** An editable of the page's own, for focus and typing around the anchor's steps. */
function editor(text = ''): HTMLElement {
  const el = document.createElement('div');
  el.setAttribute('contenteditable', 'true');
  el.textContent = text;
  document.body.appendChild(el);
  return el;
}

beforeEach(() => {
  stack = installNativeUndoStack();
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  stack.uninstall();
  document.body.innerHTML = '';
  window.getSelection()?.removeAllRanges();
});

describe('NativeUndoAnchor', () => {
  it('stays out of the tab order and the accessibility tree', () => {
    render();

    expect(anchor().getAttribute('contenteditable')).toBe('true');
    expect(anchor().tabIndex).toBe(-1);
    expect(anchor().getAttribute('aria-hidden')).toBe('true');
  });

  it('records a step as one edit of its own, and runs it as undo and redo walk the stack', () => {
    render();
    const log: string[] = [];

    expect(record('a', log)).toBe(true);
    expect(anchor().textContent).toHaveLength(1);
    // Recording is not an undo or a redo.
    expect(log).toEqual([]);

    expect(stack.undo()).toBe(true);
    expect(anchor().textContent).toBe('');
    expect(stack.redo()).toBe(true);
    expect(anchor().textContent).toHaveLength(1);
    expect(log).toEqual(['undo a', 'redo a']);
  });

  it('keeps its steps in order with the typing before and after them', () => {
    render();
    const input = editor();
    const log: string[] = [];

    stack.type(input, 'abc');
    record('a', log);
    stack.type(input, 'def');

    stack.undo();
    expect(input.textContent).toBe('abc');
    expect(log).toEqual([]);
    stack.undo();
    expect(input.textContent).toBe('abc');
    expect(log).toEqual(['undo a']);
    stack.undo();
    expect(input.textContent).toBe('');

    stack.redo();
    expect(input.textContent).toBe('abc');
    expect(log).toEqual(['undo a']);
    stack.redo();
    expect(log).toEqual(['undo a', 'redo a']);
    stack.redo();
    expect(input.textContent).toBe('abcdef');
  });

  it('undoes the newest step first, and drops the undone ones when another is recorded', () => {
    render();
    const log: string[] = [];
    record('a', log);
    record('b', log);

    stack.undo();
    stack.undo();
    expect(log).toEqual(['undo b', 'undo a']);
    stack.redo();
    expect(log).toEqual(['undo b', 'undo a', 'redo a']);

    // Recorded over the undone `b`, as a new edit drops the browser's redo.
    record('c', log);
    expect(stack.redo()).toBe(false);
    stack.undo();
    stack.undo();
    expect(log).toEqual(['undo b', 'undo a', 'redo a', 'undo c', 'undo a']);
  });

  it('runs every step between where the stack stood and where it stands', () => {
    render();
    const log: string[] = [];
    record('a', log);
    record('b', log);

    // A browser that walks both edits as one (WebKit groups those made
    // while handling the same event) reports the pair in a single change.
    anchor().textContent = '';
    anchor().dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'historyUndo' }));
    expect(log).toEqual(['undo b', 'undo a']);
  });

  it('gives focus and the caret back after recording', () => {
    render();
    const input = editor('some text');
    input.focus();
    const text = input.firstChild!;
    window.getSelection()!.setBaseAndExtent(text, 4, text, 4);

    record('a', []);

    expect(document.activeElement).toBe(input);
    const selection = window.getSelection()!;
    expect(selection.anchorNode).toBe(text);
    expect(selection.anchorOffset).toBe(4);
  });

  it('takes no focus to keep, and leaves no selection of its own, when recording with none', () => {
    render();

    record('a', []);

    expect(document.activeElement).toBe(document.body);
    const selection = window.getSelection()!;
    expect(anchor().contains(selection.anchorNode)).toBe(false);
  });

  it('gives focus back before a step runs, so the step finds it where the user had it', () => {
    render();
    const input = editor('some text');
    let focusedDuringUndo: Element | null = null;
    component!.record({
      undo: () => (focusedDuringUndo = document.activeElement),
      redo: () => {},
    });
    input.focus();
    const text = input.firstChild!;
    window.getSelection()!.setBaseAndExtent(text, 2, text, 2);

    // The browser moves focus to the anchor as it undoes the edit there.
    stack.undo();

    expect(focusedDuringUndo).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(window.getSelection()!.anchorOffset).toBe(2);
  });

  it('keeps its edits to itself', () => {
    render();
    const heard = vi.fn();
    target.addEventListener('input', heard);

    record('a', []);
    stack.undo();

    expect(heard).not.toHaveBeenCalled();
  });

  it('reports a step it could not record, and runs nothing for it', () => {
    stack.uninstall();
    render();
    const log: string[] = [];

    expect(record('a', log)).toBe(false);
    expect(anchor().textContent).toBe('');
    expect(log).toEqual([]);
  });
});
