<!--
  NativeUndoAnchor.svelte — Puts steps of the app's own on the browser's undo
  stack, in order with the text edits around them.

  A browser only records edits to editable content, so a change kept in
  component state (a block removed from a draft) is invisible to Cmd+Z, the
  Edit menu's Undo and a phone's undo gesture. `record` gives such a change
  an edit to stand for it: one invisible character typed into this hidden
  editable element. The browser's stack is the document's, shared by every
  editable in it, so that edit sits between the typing before and after it.
  Undoing it takes the character out again and redoing it puts it back,
  each with an `input` event here; the length of the element's text is then
  how many recorded steps stand, and the steps on either side of the change
  are run to match.

  Recording has to put the selection in the element, which takes focus for
  the length of the call; undo and redo move it here too, as they do to any
  edit. Either way focus and selection go back where they were before the
  steps run, so a step sees the page as the user left it.
-->
<script lang="ts" module>
  /** A change to app state the browser's undo stack is to carry. */
  export type NativeUndoStep = {
    /** Take the change back. */
    undo: () => void;
    /** Make it again, after an undo. */
    redo: () => void;
  };

  /** What each recorded step types: nothing to see, should the text ever be copied. */
  const MARK = '​';

  type FocusSnapshot = {
    element: HTMLElement | null;
    /** The selection as anchor node, anchor offset, focus node, focus offset. */
    range: [Node, number, Node, number] | null;
  };
</script>

<script lang="ts">
  import { isTextInput } from './textInput';

  let anchorEl = $state<HTMLDivElement>();
  /** The recorded steps, oldest first; those from `depth` on have been undone. */
  let steps: NativeUndoStep[] = [];
  let depth = 0;
  /** `record` is typing its mark: the `input` it raises is not an undo or redo. */
  let recording = false;
  /** Where focus was as an undo or redo began, before the browser moved it here. */
  let heldFocus: FocusSnapshot | null = null;

  function inAnchor(node: Node | null): boolean {
    return !!node && !!anchorEl?.contains(node);
  }

  /**
   * Note where focus and the selection are, ahead of their move here. With
   * no text input focused there is no on-screen keyboard up, and the element
   * asks for none, so taking focus for a moment doesn't raise one.
   */
  function holdFocus(): FocusSnapshot {
    const active = document.activeElement;
    const element =
      active instanceof HTMLElement && active !== document.body && active !== anchorEl
        ? active
        : null;
    const selection = window.getSelection();
    const range: FocusSnapshot['range'] =
      selection && selection.rangeCount > 0 && selection.anchorNode && selection.focusNode
        ? [selection.anchorNode, selection.anchorOffset, selection.focusNode, selection.focusOffset]
        : null;
    if (anchorEl) anchorEl.inputMode = isTextInput(element) ? 'text' : 'none';
    return { element, range: range && !inAnchor(range[0]) ? range : null };
  }

  /**
   * Give focus and the selection back. A selection is restored only where
   * it was the page's own or inside the focused element: a text control
   * keeps its caret itself, and a range set outside it would take its focus.
   */
  function returnFocus(held: FocusSnapshot) {
    if (held.element?.isConnected) held.element.focus({ preventScroll: true });
    else anchorEl?.blur();
    const selection = window.getSelection();
    if (!selection) return;
    const range = held.range;
    const within = (node: Node) =>
      node.isConnected && (!held.element || held.element.contains(node));
    if (range && within(range[0]) && within(range[2])) selection.setBaseAndExtent(...range);
    else if (inAnchor(selection.anchorNode)) selection.removeAllRanges();
  }

  /**
   * Put `step` on the browser's undo stack as the newest edit. Returns
   * whether it was recorded: false where the browser can't type into the
   * element, and the change then stands with no undo.
   */
  export function record(step: NativeUndoStep): boolean {
    const selection = window.getSelection();
    if (!anchorEl || !selection || typeof document.execCommand !== 'function') return false;
    const held = holdFocus();
    recording = true;
    try {
      anchorEl.focus({ preventScroll: true });
      selection.selectAllChildren(anchorEl);
      selection.collapseToEnd();
      document.execCommand('insertText', false, MARK);
    } finally {
      recording = false;
      returnFocus(held);
    }
    if (anchorEl.textContent.length !== depth + 1) return false;
    // Recording drops whatever had been undone, as the browser's stack does.
    steps = [...steps.slice(0, depth), step];
    depth = steps.length;
    return true;
  }

  /** An undo or redo is about to change the element, and to move focus here. */
  function handleBeforeInput() {
    if (!recording) heldFocus = holdFocus();
  }

  /**
   * An undo or redo changed the element: run the steps between where the
   * stack stood and where it stands now. Focus goes back first, so each
   * step finds it where the user had it.
   */
  function handleInput(event: Event) {
    // The element's edits are this component's own business: nothing above
    // it should take them for the user's typing.
    event.stopPropagation();
    if (recording || !anchorEl) return;
    const held = heldFocus ?? { element: null, range: null };
    heldFocus = null;
    if (document.activeElement === anchorEl) returnFocus(held);
    const standing = Math.min(anchorEl.textContent.length, steps.length);
    while (depth > standing) steps[--depth].undo();
    while (depth < standing) steps[depth++].redo();
  }
</script>

<!-- Out of the tab order and the accessibility tree: nothing here is for the
     user to reach or read. `input` is heard on the element itself (a capture
     listener is not delegated), so stopping it there keeps it from every
     listener above. -->
<div
  bind:this={anchorEl}
  class="native-undo-anchor"
  contenteditable="true"
  tabindex="-1"
  aria-hidden="true"
  spellcheck="false"
  onbeforeinput={handleBeforeInput}
  oninputcapture={handleInput}
></div>

<style>
  /* Rendered, as only rendered content is editable, but with nothing to see
     or press. In its host's corner rather than off screen, so the browser
     showing the edit it undid has nowhere to scroll to. */
  .native-undo-anchor {
    position: absolute;
    top: 0;
    left: 0;
    width: 1px;
    height: 1px;
    overflow: hidden;
    opacity: 0;
    pointer-events: none;
    white-space: pre;
    outline: none;
  }
</style>
