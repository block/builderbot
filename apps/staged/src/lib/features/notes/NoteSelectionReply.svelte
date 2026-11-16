<!--
  NoteSelectionReply.svelte — "Select to reply" for a rendered note.

  Selecting text in the note offers a Reply button: by the end of the
  selection, or on a phone (`dockHost`, see below) docked above the note's
  bottom bar. Pressing it quotes the selection into the note chat's draft as
  a quote block; nothing has to be confirmed afterwards.

  - With the chat composer showing beside the note (`chatVisible`), the block
    goes straight into it (`onAdd`) and the caret lands in its reply input.
  - Otherwise a reply field opens here — a popover where the button was,
    or on a phone a sheet at the bottom of the dialog (`sheetHost`), above
    the keyboard. The button goes while the field is open: it has been
    pressed, and comes back with the next selection. The popover sits by
    the passage, which is lit in the note, so it holds only the reply; the
    sheet lies over the note and may cover the passage, so it shows the
    quote and its heading path as well. The block is already in
    the draft when the field opens (`onReply`, see
    replyDrafts.startLiveQuoteReply), and the reply typed into it is written
    as it goes, so dismissing the field only closes it: Escape, Enter, a
    click or tap away, scrolling the note, or the chat opening (`chatOpen`,
    beside the note or over it). An empty reply leaves the quote block in
    the draft all the same, and the user stays on the note, free to select
    the next passage; the bottom bar's draft button is the way to the chat.

  Each selection makes one block, so a review of the whole note builds up
  one selection at a time. The block carries where it was selected
  (noteQuoteLocation.captureQuoteAnchor), which is what lets the note mark
  the passage afterwards (NoteQuoteHighlights) and tell it from the same
  words elsewhere.

  Pressing a marked passage reopens this field on its block (`editQuote`):
  the same popover or sheet, by the passage, holding the reply written so
  far. What is typed updates that block (`onEdit`, see
  replyDrafts.editLiveQuoteReply) rather than adding another, and closing
  works as above. Reopened, the field also has a Remove button, which takes
  the block out of the draft; a new reply's field has nothing to press, as
  before. Selecting exactly the passage a block already quotes and pressing
  Reply reopens that block the same way (`existingQuote`), instead of
  putting a second block on the same words. A block that leaves the draft
  from elsewhere while its field is open takes the field with it
  (`closeQuote`).

  A field opened by a key press on a mark gives focus back to that quote's
  mark when a key closes it: the mark as the note has it then, since the
  marks are rebuilt whenever the note re-renders. When the mark is gone
  (Remove, or the passage no longer in the note) the dialog (`sheetHost`)
  takes focus, so the keyboard is not left on an element that has gone.

  Rendered inside the note's scroll container (`scrollEl`, which must be
  positioned), so the button scrolls with the text it belongs to. While the
  field is open the selection is frozen — focus moving into the field drops
  the page selection — and painted with a CSS highlight in its place, until
  the note marks the passage itself (`markedQuoteIds`), which it does once
  the block is in the draft. The
  quote is captured when the button appears, so the note re-rendering under
  an open field (an agent editing it) leaves the field, and the reply being
  typed, where they are; only the highlight goes with the old text. That
  holds across `enabled` dropping for the re-render too (the note is
  unrendered while a new image's dimensions or the diagram renderer load):
  an open field waits it out; only a closed one is cleared.

  By touch the button appears once the selection's handles have been still
  for a moment (iOS sends no pointer events for the gestures that select
  text), and a tap on it collapses the page selection before its click: the
  captured quote is kept through the press so the tap still lands. On a
  phone (`viewport.sheetMenus`) the button is not by the selection at all:
  iOS draws its edit menu (Copy, Look Up, …) right there, over anything
  else. It is a full-width "Reply to selection" row instead, docked in
  `dockHost` over the note's bottom bar, with a line of the selected text,
  shown while a selection stands and withdrawn as it collapses
  (`onDockedChange` lets the bar hide under it). A tablet with a coarse pointer
  but no sheet menus keeps the button by the selection, clear of the end
  handle's knob (see replyButtonOffset).
-->
<script lang="ts">
  import { flushSync, onDestroy, tick, untrack } from 'svelte';
  import { Portal } from 'bits-ui';
  import MessageSquareQuote from '@lucide/svelte/icons/message-square-quote';
  import PencilLine from '@lucide/svelte/icons/pencil-line';
  import Trash2 from '@lucide/svelte/icons/trash-2';
  import * as Popover from '$lib/components/ui/popover';
  import { createScrimDismiss } from '$lib/components/ui/menu-sheet/scrimDismiss';
  import QuoteExcerpt from '../sessions/QuoteExcerpt.svelte';
  import type { DraftQuote, NewDraftQuote } from '../sessions/composerDraft';
  import type { LiveQuoteReply } from '../sessions/replyDrafts';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import { viewport } from '../../shared/viewport.svelte';
  import { quoteHighlightRange, quoteHighlightStop } from './noteQuoteHighlights';
  import { captureQuoteAnchor } from './noteQuoteLocation';
  import { quoteFromSelection, replyButtonOffset, type NoteSelectionQuote } from './noteSelection';

  interface Props {
    /** The note's scroll container; the button is positioned in it. */
    scrollEl: HTMLElement | undefined;
    /** The rendered note, whose selections can be replied to. */
    contentEl: HTMLElement | undefined;
    /** Off while the note is hidden or has no chat to reply in. */
    enabled: boolean;
    /** The chat is open, beside the note or over it: an open field closes and the composer has the block. */
    chatOpen: boolean;
    /** The chat composer shows the draft beside the note: quotes go straight into it. */
    chatVisible: boolean;
    /**
     * The dialog around the note. On a phone the reply field is a sheet in
     * it rather than a popover; at any size it takes focus when a closing
     * field has no mark to give it back to.
     */
    sheetHost?: HTMLElement | null;
    /** On a phone the Reply button is docked in this element (above the note's bottom bar). */
    dockHost?: HTMLElement | null;
    /** The docked button came or went (it is shown while a selection stands). */
    onDockedChange?: (docked: boolean) => void;
    /** Headings left out of the path (the note's title). */
    ignoreHeading?: (heading: Element) => boolean;
    /** Chat visible: add the block and focus its reply. Resolves whether it was added. */
    onAdd: (quote: NewDraftQuote) => Promise<boolean> | boolean;
    /** Chat hidden: add the block and keep its reply current as it is typed here. */
    onReply: (quote: NewDraftQuote) => LiveQuoteReply;
    /** Chat hidden: keep the reply of a block already in the draft current as it is edited here. */
    onEdit?: (quote: DraftQuote) => LiveQuoteReply;
    /** Chat hidden: the block already quoting exactly the passage `quote` was selected at, if any. */
    existingQuote?: (quote: NewDraftQuote) => DraftQuote | null;
    /** The quotes the note marks in its text (NoteQuoteHighlights). */
    markedQuoteIds?: readonly string[];
    /** The block the open field writes the reply of, or null with no field open. */
    onActiveQuoteChange?: (quoteId: string | null) => void;
  }

  let {
    scrollEl,
    contentEl,
    enabled,
    chatOpen,
    chatVisible,
    sheetHost = null,
    dockHost = null,
    onDockedChange,
    ignoreHeading,
    onAdd,
    onReply,
    onEdit,
    existingQuote,
    markedQuoteIds = [],
    onActiveQuoteChange,
  }: Props = $props();

  /** Wait this long after the last selection change before showing the button. */
  const SETTLE_MS = 150;
  /** By touch the handles are dragged in stops and starts; wait a little longer. */
  const TOUCH_SETTLE_MS = 350;
  /** How long a press on the button may take to reach its click. */
  const PRESS_MS = 600;
  /** A wheel this soon after the field opened is the trackpad's inertia, not a scroll away. */
  const WHEEL_GRACE_MS = 300;
  /** The docked button's leaving animation (see the stylesheet). */
  const DOCK_LEAVE_MS = 150;
  const HIGHLIGHT_NAME = 'note-reply-selection';

  let selection = $state.raw<NoteSelectionQuote | null>(null);
  let anchor = $state<{ top: number; left: number } | null>(null);
  let open = $state(false);
  let reply = $state('');
  /** `onAdd` is still putting the block in the composer. */
  let adding = $state(false);
  /** The block the open field is typing the reply of. */
  let live: LiveQuoteReply | null = null;
  /** That block's id. */
  let activeQuoteId = $state<string | null>(null);
  /** The field was opened on a block already in the draft (`editQuote`), not by a selection. */
  let editing = $state(false);
  /** A key press on the block's mark opened the field: the mark takes focus back as a key closes it. */
  let openedByKey = false;
  let replyEl = $state<HTMLTextAreaElement | null>(null);
  /** Where the button is, and the popover while the button is away. */
  let anchorEl = $state<HTMLElement | null>(null);
  let buttonEl = $state<HTMLElement | null>(null);
  /**
   * The button's width as last measured. It is not mounted under an open
   * field, which stays where the button's width put it.
   */
  let buttonWidth: number | null = null;
  let dockButtonEl = $state<HTMLElement | null>(null);
  /** The selection the docked button shows; kept through its leaving animation. */
  let dockShown = $state.raw<NoteSelectionQuote | null>(null);
  let dockLeaving = $state(false);
  let dockLeaveTimer: ReturnType<typeof setTimeout> | undefined;
  /** A mouse drag is selecting; the button waits for its release. */
  let mouseDown = false;
  /** A press on the button is under way (see the header). */
  let pressingButton = false;
  let settleTimer: ReturnType<typeof setTimeout> | undefined;
  let pressTimer: ReturnType<typeof setTimeout> | undefined;

  let sheetMode = $derived(viewport.sheetMenus && sheetHost !== null);
  let dockMode = $derived(viewport.sheetMenus && dockHost !== null);
  /** The selected text on one line, for the docked button. */
  let dockExcerpt = $derived(dockShown ? dockShown.quote.replace(/\s+/g, ' ').trim() : '');
  // A tap on the scrim is the user's own dismissal: the selection goes with it.
  const scrim = createScrimDismiss(() => closeReply(true));

  function clearSelectionState() {
    selection = null;
    anchor = null;
  }

  /**
   * Where the button goes, in scroll coordinates. None for a collapsed range
   * — the selected text was replaced, and the range fell back to where it
   * had been.
   */
  function anchorFor(range: Range, container: HTMLElement): { top: number; left: number } | null {
    if (range.collapsed) return null;
    const rects = [...range.getClientRects()].filter((rect) => rect.width > 0 && rect.height > 0);
    const last = rects[rects.length - 1] ?? range.getBoundingClientRect();
    if (last.width === 0 && last.height === 0) return null;
    const touch = viewport.hasCoarsePointer;
    if (buttonEl) buttonWidth = buttonEl.offsetWidth;
    const box = container.getBoundingClientRect();
    return replyButtonOffset({
      end: last,
      container: {
        top: box.top,
        left: box.left,
        scrollTop: container.scrollTop,
        scrollLeft: container.scrollLeft,
        clientWidth: container.clientWidth,
      },
      buttonWidth: buttonWidth ?? (touch ? 92 : 72),
      touch,
    });
  }

  function readSelection() {
    clearTimeout(settleTimer);
    if (open) return;
    if (!enabled || !contentEl || !scrollEl) {
      clearSelectionState();
      return;
    }
    const next = quoteFromSelection(document.getSelection(), contentEl, { ignore: ignoreHeading });
    if (!next) {
      clearSelectionState();
      return;
    }
    selection = next;
    anchor = anchorFor(next.range, scrollEl);
  }

  function handleSelectionChange() {
    if (open) return;
    const current = document.getSelection();
    // Gone (or moved out of the note): hide at once. Still changing: wait for
    // the drag or the keyboard to settle before showing the button.
    if (!current || current.isCollapsed || !contentEl?.contains(current.anchorNode)) {
      clearTimeout(settleTimer);
      // A touch on the button collapses the page selection before the
      // button's click: the captured quote is what the click quotes.
      if (pressingButton) return;
      clearSelectionState();
      return;
    }
    clearTimeout(settleTimer);
    if (mouseDown) return;
    settleTimer = setTimeout(
      readSelection,
      viewport.hasCoarsePointer ? TOUCH_SETTLE_MS : SETTLE_MS
    );
  }

  function isReplyButton(target: EventTarget | null): boolean {
    const node = target as Node | null;
    return !!node && (!!buttonEl?.contains(node) || !!dockButtonEl?.contains(node));
  }

  function handlePointerDown(event: PointerEvent) {
    if (isReplyButton(event.target)) {
      pressingButton = true;
      clearTimeout(pressTimer);
      pressTimer = setTimeout(endPress, PRESS_MS);
      return;
    }
    // Only a mouse drag holds the button back until its release: iOS sends
    // no pointerup for a long press that selects, or for dragging a handle.
    if (event.pointerType === 'mouse') mouseDown = true;
  }

  function handlePointerUp() {
    if (!mouseDown) return;
    mouseDown = false;
    // After the browser has settled the selection the press made.
    setTimeout(readSelection, 0);
  }

  /** The press ended without its click (dragged off, cancelled, or just slow). */
  function endPress() {
    clearTimeout(pressTimer);
    pressTimer = undefined;
    if (!pressingButton) return;
    pressingButton = false;
    if (!open && !adding) readSelection();
  }

  $effect(() => {
    if (!enabled) {
      // An open field rides out a transient drop (the note between renders)
      // with its reply; closing it later goes through `closeReply`. With
      // none open there is nothing to keep, the reply included, so a later
      // field starts blank.
      if (!untrack(() => open)) {
        clearSelectionState();
        reply = '';
      }
      return;
    }
    document.addEventListener('selectionchange', handleSelectionChange);
    document.addEventListener('pointerdown', handlePointerDown, true);
    document.addEventListener('pointerup', handlePointerUp, true);
    document.addEventListener('pointercancel', handlePointerUp, true);
    return () => {
      document.removeEventListener('selectionchange', handleSelectionChange);
      document.removeEventListener('pointerdown', handlePointerDown, true);
      document.removeEventListener('pointerup', handlePointerUp, true);
      document.removeEventListener('pointercancel', handlePointerUp, true);
      clearTimeout(settleTimer);
    };
  });

  // Text reflowing under the selection (the dialog resized, the chat pane
  // opened beside it) moves the selection's end; the button follows. The
  // note re-rendering replaces the selected text and leaves no end to
  // follow: an open field then stays where it is rather than unmounting
  // with the reply in it.
  $effect(() => {
    const el = contentEl;
    if (!el || !enabled) return;
    const observer = new ResizeObserver(() => {
      if (!selection || !scrollEl) return;
      // Marking the passage wraps its text, which leaves the captured range
      // short of it: the mark is where the passage is from then on.
      const marked = open && activeQuoteId ? quoteHighlightRange(el, activeQuoteId) : null;
      const next = anchorFor(marked ?? selection.range, scrollEl);
      if (next || !open) anchor = next;
    });
    observer.observe(el);
    return () => observer.disconnect();
  });

  // Paint the frozen selection while the field holds it, until the note
  // marks the passage itself: its mark is lit for the open field instead.
  $effect(() => {
    const marked = activeQuoteId !== null && markedQuoteIds.includes(activeQuoteId);
    const range = open && !marked ? selection?.range : null;
    const registry = globalThis.CSS?.highlights;
    if (!range || !registry || typeof Highlight === 'undefined') return;
    registry.set(HIGHLIGHT_NAME, new Highlight(range));
    return () => {
      registry.delete(HIGHLIGHT_NAME);
    };
  });

  // The docked button follows the selection, but leaves with a short
  // animation: the selection it showed is kept until that has run.
  $effect(() => {
    // Not for a field reopened from a mark: there is no selection to offer.
    const current = dockMode && anchor && !editing ? selection : null;
    if (current) {
      clearTimeout(dockLeaveTimer);
      dockShown = current;
      dockLeaving = false;
      return;
    }
    if (!untrack(() => dockShown)) return;
    dockLeaving = true;
    dockLeaveTimer = setTimeout(() => {
      dockShown = null;
      dockLeaving = false;
    }, DOCK_LEAVE_MS);
    return () => clearTimeout(dockLeaveTimer);
  });

  $effect(() => {
    const docked = dockShown !== null;
    untrack(() => onDockedChange?.(docked));
  });

  $effect(() => {
    const quoteId = activeQuoteId;
    untrack(() => onActiveQuoteChange?.(quoteId));
  });

  // The chat opening (beside the note, or over it on a phone) takes the
  // block over: the field closes and the reply so far goes with it.
  $effect(() => {
    if (!chatOpen) return;
    untrack(() => closeReply(true));
  });

  // Scrolling the note away from the selection closes the field. A wheel
  // (the user's own scrolling) rather than `scroll`: the keyboard opening
  // can shorten the note's box and clamp its scroll under the field. Not
  // the first moments, though: a trackpad still coasting from the scroll
  // that preceded the selection would close the field as it opened.
  $effect(() => {
    const el = scrollEl;
    if (!open || !el) return;
    let armed = false;
    const arm = setTimeout(() => {
      armed = true;
    }, WHEEL_GRACE_MS);
    const close = () => {
      if (armed) closeReply(true);
    };
    el.addEventListener('wheel', close, { passive: true });
    return () => {
      clearTimeout(arm);
      el.removeEventListener('wheel', close);
    };
  });

  onDestroy(() => {
    clearTimeout(settleTimer);
    clearTimeout(pressTimer);
    clearTimeout(dockLeaveTimer);
    // The dialog closing on an open field: whatever was typed still lands.
    const pending = live;
    live = null;
    if (pending) void pending.finish();
  });

  /** The selection has been quoted; nothing is left to reply to. */
  function finishSelection() {
    clearSelectionState();
    document.getSelection()?.removeAllRanges();
  }

  async function handleButtonClick() {
    clearTimeout(pressTimer);
    pressTimer = undefined;
    pressingButton = false;
    const current = selection;
    if (!current || adding || open) return;
    const quote: NewDraftQuote = { quote: current.quote, headings: current.headings, reply: '' };
    const quoteAnchor = contentEl
      ? captureQuoteAnchor(contentEl, current.range, current.quote)
      : null;
    if (quoteAnchor) quote.anchor = quoteAnchor;
    // The passage already has a block: its reply is reopened, rather than a
    // second block put on the same words, of which the note could only ever
    // reopen one. (With the chat visible `onAdd` sees to the same.)
    const existing = chatVisible ? null : (existingQuote?.(quote) ?? null);
    if (existing && editQuote(existing, current.range)) return;
    if (chatVisible) {
      adding = true;
      let added = false;
      try {
        added = await onAdd(quote);
      } catch (error) {
        console.error('[NoteSelectionReply] Failed to add the quote:', error);
      } finally {
        adding = false;
      }
      // Not added (`onAdd` has said why): the button stays for another try.
      if (!added || selection !== current) return;
      finishSelection();
      return;
    }
    reply = '';
    live = onReply(quote);
    activeQuoteId = live.id;
    editing = false;
    open = true;
    if (sheetMode) {
      // Mounted and focused within the tap's own task, which is what lets
      // iOS raise the keyboard for it.
      flushSync();
      replyEl?.focus();
    }
  }

  /**
   * Reopen the field on a block already in the draft, by its marked passage
   * (`range`), with the reply written so far. `byKey` when a key press on
   * the mark asked for it: the mark gets focus back when a key closes the
   * field. Returns whether it opened: not while the note is hidden or a
   * block is still being added, or when the passage has no place on screen.
   */
  export function editQuote(quote: DraftQuote, range: Range, byKey = false) {
    if (!enabled || !scrollEl || !onEdit || adding) return false;
    // Another field's reply is written before this one opens.
    closeReply(false);
    clearTimeout(settleTimer);
    const at = anchorFor(range, scrollEl);
    if (!at && !sheetMode) return false;
    selection = { quote: quote.quote, headings: quote.headings, range };
    anchor = at;
    reply = quote.reply;
    live = onEdit(quote);
    activeQuoteId = quote.id;
    editing = true;
    openedByKey = byKey;
    // The anchor the popover hangs from is mounted before the popover opens.
    flushSync();
    open = true;
    if (sheetMode) {
      flushSync();
      replyEl?.focus();
    }
    return true;
  }

  /**
   * The block the open field writes to has left the draft (removed in the
   * composer, or sent, from somewhere else): the field closes, rather than
   * go on saying the reply is kept while nothing typed in it is.
   */
  export function closeQuote(quoteId: string) {
    if (open && activeQuoteId === quoteId) closeReply(false, false, true);
  }

  /** Whether focus is in the open field (its input or its Remove button). */
  function fieldHasFocus(): boolean {
    const active = document.activeElement;
    return !!active && !!replyEl?.parentElement?.contains(active);
  }

  /**
   * Close the field. The block stays in the draft with the reply typed so
   * far (`finish` writes what is still pending). `clearPageSelection` when
   * the user's own action closed it; a pointer press elsewhere may be the
   * start of the next selection, which is left alone. `byKey` when a key in
   * the field closed it: a field a key press opened from a mark then gives
   * focus back to that mark. `gone` when the block has left the draft, or is
   * leaving it, and its mark with it: focus in the field then goes to the
   * dialog.
   */
  function closeReply(clearPageSelection: boolean, byKey = false, gone = false) {
    if (!open) return;
    // Read before anything below moves focus or unmounts the field.
    const toMark = byKey && openedByKey && !gone;
    const moveFocus = toMark || (gone && fieldHasFocus());
    // The sheet unmounting with focus in it would leave the dialog's focus
    // trap to pick a control (its header's first button); the dialog itself
    // takes focus instead, as it does when a tap on the scrim dismisses.
    if (sheetMode && sheetHost?.contains(document.activeElement)) {
      sheetHost.focus({ preventScroll: true });
    }
    open = false;
    const pending = live;
    live = null;
    if (pending) void pending.finish();
    reply = '';
    const quoteId = activeQuoteId;
    activeQuoteId = null;
    editing = false;
    openedByKey = false;
    if (clearPageSelection) finishSelection();
    else clearSelectionState();
    if (moveFocus) {
      // Once the field is closed: its popover holds focus inside it until
      // then. The mark is looked up now, not kept from when the field
      // opened: the note's marks are rebuilt on every render and every
      // change of the draft's quotes, which leaves an element kept from
      // before out of the document. Failing a mark, the dialog.
      void tick().then(() => {
        const stop = toMark && contentEl && quoteId ? quoteHighlightStop(contentEl, quoteId) : null;
        (stop ?? sheetHost)?.focus({ preventScroll: true });
      });
    }
  }

  /** Take the open field's block out of the draft, and close the field. */
  function removeReply() {
    const pending = live;
    if (!open || !pending) return;
    // Removed rather than finished: what was typed goes with the block.
    live = null;
    void pending.remove();
    closeReply(true, false, true);
  }

  function handleOpenChange(next: boolean) {
    if (!next) closeReply(false);
  }

  function handleReplyInput(event: Event) {
    const value = (event.currentTarget as HTMLTextAreaElement).value;
    reply = value;
    live?.setReply(value);
  }

  function handleReplyKeydown(event: KeyboardEvent) {
    if (isImeKeydown(event)) return;
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      closeReply(true, true);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      // Not the note dialog's to close as well: its escape layer listens on
      // the document and would take this key from the sheet. The popover's
      // own layer listens there too, so the field closes here, not through
      // bits-ui; the selection goes with it, as the user asked.
      event.stopPropagation();
      closeReply(true, true);
    }
  }
</script>

<!-- `withQuote` for the sheet, which may cover the passage it replies to. -->
{#snippet replyField(withQuote: boolean)}
  {#if withQuote}
    <QuoteExcerpt quote={selection!.quote} headings={selection!.headings} clampLines={3} />
  {/if}
  <textarea
    bind:this={replyEl}
    value={reply}
    class="selection-reply-input"
    rows="2"
    placeholder="Reply to this selection…"
    aria-label="Reply to the selected text"
    oninput={handleReplyInput}
    onkeydown={handleReplyKeydown}></textarea>
  <div class="selection-reply-status">
    <PencilLine size={11} aria-hidden="true" />
    <span>
      Kept in your reply draft{#if viewport.showShortcutHints}
        · Enter or Esc to close{/if}
    </span>
    {#if editing}
      <button
        type="button"
        class="selection-reply-remove touch-hit"
        title="Remove this quote from your reply draft"
        aria-label="Remove this quote from your reply draft"
        onclick={removeReply}
      >
        <Trash2 size={11} aria-hidden="true" />
        <span>Remove</span>
      </button>
    {/if}
  </div>
{/snippet}

{#if selection && anchor && !dockMode}
  <div
    bind:this={anchorEl}
    class="selection-reply-anchor"
    style:top="{anchor.top}px"
    style:left="{anchor.left}px"
  >
    <!-- Not under an open field (`editing`: one about to open from a mark),
         which takes the button's place. -->
    {#if !open && !editing}
      <button
        type="button"
        bind:this={buttonEl}
        class="selection-reply-button touch-hit"
        title="Reply to the selected text"
        aria-label="Reply to the selected text"
        aria-busy={adding}
        onmousedown={(event: MouseEvent) => event.preventDefault()}
        onclick={handleButtonClick}
      >
        <MessageSquareQuote size={12} aria-hidden="true" />
        <span>Reply</span>
      </button>
    {/if}
    {#if !sheetMode}
      <Popover.Root {open} onOpenChange={handleOpenChange}>
        <!-- The passage is lit in the note right beside the popover, so the
             quote is not repeated in it. With no quote to read out, the
             label is what names the popover. -->
        <Popover.Content
          customAnchor={anchorEl}
          side="bottom"
          align="start"
          sideOffset={0}
          collisionPadding={12}
          role="dialog"
          aria-label="Reply to the selected text"
          class="selection-reply-popover w-[min(340px,calc(100vw-24px))] gap-2 p-2.5"
          onOpenAutoFocus={(event: Event) => {
            event.preventDefault();
            replyEl?.focus();
          }}
          onCloseAutoFocus={(event: Event) => event.preventDefault()}
        >
          {@render replyField(false)}
        </Popover.Content>
      </Popover.Root>
    {/if}
  </div>
{/if}

{#if dockMode && dockShown}
  <Portal to={dockHost!}>
    <div class="selection-reply-dock" class:is-leaving={dockLeaving}>
      <button
        type="button"
        bind:this={dockButtonEl}
        class="selection-reply-dock-button"
        class:is-open={open}
        aria-busy={adding}
        onmousedown={(event: MouseEvent) => event.preventDefault()}
        onclick={handleButtonClick}
      >
        <MessageSquareQuote size={18} aria-hidden="true" />
        <span class="selection-reply-dock-text">
          <span class="selection-reply-dock-label">Reply to selection</span>
          <span class="selection-reply-dock-excerpt" aria-hidden="true">{dockExcerpt}</span>
        </span>
      </button>
    </div>
  </Portal>
{/if}

{#if sheetMode && open && selection}
  <Portal to={sheetHost!}>
    <div class="selection-reply-sheet" role="dialog" aria-label="Reply to the selected text">
      <!-- Decorative: Escape and Enter in the field are the accessible dismissal. -->
      <div
        class="selection-reply-scrim"
        aria-hidden="true"
        onpointerdown={scrim.onpointerdown}
        onpointercancel={scrim.onpointercancel}
        onclick={scrim.onclick}
      ></div>
      <div class="selection-reply-panel">
        {@render replyField(true)}
      </div>
    </div>
  </Portal>
{/if}

<style>
  .selection-reply-anchor {
    position: absolute;
    z-index: 2;
  }

  .selection-reply-button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 9px;
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    background: var(--bg-chrome);
    color: var(--text-secondary);
    font-family: inherit;
    font-size: var(--size-xs);
    font-weight: 500;
    box-shadow: var(--shadow-raised);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s,
      background-color 0.1s;
  }

  .selection-reply-button:hover {
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .selection-reply-button:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }

  .selection-reply-button[aria-busy='true'] {
    cursor: progress;
    opacity: 0.7;
  }

  /* A finger's target, with the `touch-hit` area around it making up 44px. */
  @media (pointer: coarse) {
    .selection-reply-button {
      height: 32px;
      padding: 0 13px;
      gap: 6px;
      font-size: var(--size-sm);
    }

    .selection-reply-button :global(svg) {
      width: 14px;
      height: 14px;
    }
  }

  .selection-reply-input {
    width: 100%;
    padding: 6px 9px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--size-sm);
    line-height: 1.5;
    resize: none;
  }

  .selection-reply-input::placeholder {
    color: var(--text-faint);
  }

  .selection-reply-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  /* Secondary rather than muted text: muted is under 4.5:1 on the dark
     theme's popover surface (theme.ts). */
  .selection-reply-status {
    display: flex;
    align-items: center;
    gap: 5px;
    color: var(--text-secondary);
    font-size: var(--size-xs);
    line-height: 1.4;
  }

  .selection-reply-status :global(svg) {
    flex-shrink: 0;
    color: var(--ui-accent);
  }

  /* At the end of the status line, out of the way of the field. */
  .selection-reply-remove {
    display: inline-flex;
    flex-shrink: 0;
    align-items: center;
    gap: 4px;
    margin-left: auto;
    padding: 2px 6px;
    border: none;
    border-radius: 6px;
    background: none;
    color: var(--text-secondary);
    font-family: inherit;
    font-size: inherit;
    line-height: inherit;
    cursor: pointer;
  }

  .selection-reply-remove :global(svg) {
    color: currentColor;
  }

  .selection-reply-remove:hover {
    background: var(--ui-danger-bg);
    color: var(--ui-danger);
  }

  .selection-reply-remove:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }

  /* The sheet fills the dialog it is portalled into. Dialog.Content is the
     fixed box sized to the visual viewport (see dialog-content), so
     `inset: 0` here puts the panel on the keyboard's top edge; the scrim
     takes the tap that dismisses it (see scrimDismiss.ts). */
  .selection-reply-sheet {
    position: absolute;
    inset: 0;
    z-index: var(--z-index-floating);
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
  }

  .selection-reply-scrim {
    position: absolute;
    inset: 0;
    background: var(--shadow-overlay);
    animation: selection-reply-fade 0.2s ease-out;
  }

  .selection-reply-panel {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin: 0 8px;
    margin-bottom: calc(8px + var(--dialog-safe-bottom, 0px));
    padding: 12px 14px;
    border-radius: 16px;
    background: var(--popover);
    color: var(--text-primary);
    box-shadow: var(--shadow-raised);
    animation: selection-reply-rise 0.2s ease-out;
  }

  /* 16px, so iOS doesn't zoom the page to the field. */
  .selection-reply-panel .selection-reply-input {
    padding: 10px 12px;
    font-size: 16px;
    line-height: 1.4;
  }

  .selection-reply-panel .selection-reply-status {
    font-size: var(--size-sm);
  }

  @keyframes selection-reply-fade {
    from {
      opacity: 0;
    }
  }

  @keyframes selection-reply-rise {
    from {
      translate: 0 16px;
      opacity: 0;
    }
  }

  /* The docked button: a bar of the bottom bar's chrome, full width, with a
     finger-sized row in it. It rises in with the selection and falls away
     (`is-leaving`, DOCK_LEAVE_MS) as the selection goes. */
  /* The dock it is portalled into lets presses through to the bar it covers
     (NoteModal's `.selection-dock`); the row takes its own. */
  .selection-reply-dock {
    pointer-events: auto;
    padding: 8px 16px;
    padding-left: calc(16px + var(--dialog-safe-left, 0px));
    padding-right: calc(16px + var(--dialog-safe-right, 0px));
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    animation: selection-reply-dock-rise 0.15s ease-out;
  }

  .selection-reply-dock.is-leaving {
    pointer-events: none;
    animation: selection-reply-dock-fall 0.15s ease-in forwards;
  }

  .selection-reply-dock-button {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 44px;
    padding: 5px 12px;
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-family: inherit;
    text-align: left;
    cursor: pointer;
  }

  .selection-reply-dock-button:active,
  .selection-reply-dock-button.is-open {
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
  }

  .selection-reply-dock-button:focus-visible {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }

  .selection-reply-dock-button[aria-busy='true'] {
    cursor: progress;
    opacity: 0.7;
  }

  .selection-reply-dock-button :global(svg) {
    flex-shrink: 0;
    color: var(--ui-accent);
  }

  .selection-reply-dock-text {
    display: flex;
    flex-direction: column;
    gap: 1px;
    min-width: 0;
  }

  .selection-reply-dock-label {
    font-size: var(--size-sm);
    font-weight: 500;
    line-height: 1.3;
  }

  .selection-reply-dock-excerpt {
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--size-xs);
    line-height: 1.3;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  @keyframes selection-reply-dock-rise {
    from {
      translate: 0 8px;
      opacity: 0;
    }
  }

  @keyframes selection-reply-dock-fall {
    to {
      translate: 0 8px;
      opacity: 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .selection-reply-scrim,
    .selection-reply-panel,
    .selection-reply-dock,
    .selection-reply-dock.is-leaving {
      animation: none;
    }
  }

  /* The note's own tint, as on the rule beside each quote. */
  :global(::highlight(note-reply-selection)) {
    background-color: var(--note-bg-emphasis);
  }
</style>
