/**
 * A stand-in for the browser's undo stack, which jsdom doesn't have.
 *
 * A browser keeps one stack per document for the edits made to its editable
 * content: `document.execCommand('insertText')` and typing put an edit on
 * it, and undo and redo walk it, each time changing the edited element and
 * raising `beforeinput` and `input` there with `historyUndo` or
 * `historyRedo`. This models as much of that as the app's undo wiring
 * leans on (see NativeUndoAnchor.svelte), so component tests can type,
 * undo and redo across several editable elements in the order a browser
 * would:
 *
 * - `execCommand('insertText')` appends to the editable element holding the
 *   selection, and drops whatever had been undone;
 * - undo and redo move focus and the caret to the element they change, as
 *   a browser does, between its `beforeinput` and its `input`;
 * - an edit to an element since taken out of the document is still walked,
 *   but changes nothing and raises no event.
 */

type Edit = { host: HTMLElement; before: string; after: string };

export type NativeUndoStack = {
  /** Type `text` at the end of `host`, as one edit. */
  type(host: HTMLElement, text: string): void;
  /** Undo the newest edit; false when there is none. */
  undo(): boolean;
  /** Redo the edit undone last; false when there is none. */
  redo(): boolean;
  /** Put back the `document.execCommand` that was there before. */
  uninstall(): void;
};

function caretToEnd(host: HTMLElement) {
  const selection = window.getSelection();
  selection?.selectAllChildren(host);
  selection?.collapseToEnd();
}

function editableHostOf(node: Node | null): HTMLElement | null {
  const element = node instanceof HTMLElement ? node : (node?.parentElement ?? null);
  return element?.closest<HTMLElement>('[contenteditable="true"]') ?? null;
}

export function installNativeUndoStack(): NativeUndoStack {
  const done: Edit[] = [];
  let undone: Edit[] = [];
  const original = Object.getOwnPropertyDescriptor(document, 'execCommand');

  function insertText(text: string): boolean {
    const host = editableHostOf(window.getSelection()?.anchorNode ?? null);
    if (!host) return false;
    const before = host.textContent ?? '';
    host.dispatchEvent(new InputEvent('beforeinput', { bubbles: true, inputType: 'insertText' }));
    host.textContent = before + text;
    caretToEnd(host);
    done.push({ host, before, after: before + text });
    undone = [];
    host.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText' }));
    return true;
  }

  function walk(edit: Edit, text: string, inputType: 'historyUndo' | 'historyRedo') {
    const { host } = edit;
    if (!host.isConnected) return;
    host.dispatchEvent(new InputEvent('beforeinput', { bubbles: true, inputType }));
    host.focus();
    host.textContent = text;
    caretToEnd(host);
    host.dispatchEvent(new InputEvent('input', { bubbles: true, inputType }));
  }

  Object.defineProperty(document, 'execCommand', {
    configurable: true,
    writable: true,
    value: (command: string, _showUi?: boolean, value?: string) =>
      command === 'insertText' && insertText(value ?? ''),
  });

  return {
    type(host, text) {
      host.focus();
      caretToEnd(host);
      insertText(text);
    },
    undo() {
      const edit = done.pop();
      if (!edit) return false;
      undone.push(edit);
      walk(edit, edit.before, 'historyUndo');
      return true;
    },
    redo() {
      const edit = undone.pop();
      if (!edit) return false;
      done.push(edit);
      walk(edit, edit.after, 'historyRedo');
      return true;
    },
    uninstall() {
      if (original) Object.defineProperty(document, 'execCommand', original);
      else delete (document as Partial<Document>).execCommand;
    },
  };
}
