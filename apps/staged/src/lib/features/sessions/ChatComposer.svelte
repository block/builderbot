<!--
  ChatComposer.svelte — Shared bottom chat input for session chat surfaces.

  Houses the attached-image strip, the text input, and the control row below
  it (agent config picker slot, attach button, send/queue button). Rendered by
  SessionChatPane, which backs both the session dialog and the note dialog's
  chat pane, so the composer looks and behaves identically everywhere: the
  text input gets its own full-width row with the controls beneath it.

  The submit control is the `sendControl` the pane derived with
  `composerSendControl` and acts on itself: a plain "Send" on an idle
  session, a plain "Queue" on a running one — and, when the backend reports
  the running session can take a message now, a split button whose primary
  segment performs the remembered mode (Queue / Send) and whose dropdown
  switches it. While the pane has no session row to submit to
  (`sessionBlock`: still loading, or the load settled without one) the
  control is disabled with a title saying which, but the draft can be typed.

  Where the host can start a new session from the draft (`onStartNewSession`:
  the note dialog), the button is split in every state and the dropdown also
  lists "New note" and "New commit". They are enabled exactly when the
  primary segment is, unless the host cannot start a session right now
  (`newSessionDisabled`).

  Failures of the composer's own actions (`sendError`: a refused send, a
  queue change that failed) are shown on a `ComposerError` line inside the
  composer, next to the attachment error, rather than in the pane's
  transcript slot — the transcript stays readable behind them.

  A draft can hold quote blocks — passages of a note the user selected and
  replied to (see composerDraft.ts). The text input is one field
  (`.composer-field`) that holds them all in order: the free text before each
  quote, the quote block with its reply, and the free text after the last
  one, which is the main input. The field draws the box and its focus ring;
  the inputs in it are borderless, so the whole reads as one message, and the
  field grows with its content up to a cap, then scrolls as one. ArrowUp on
  the first line of an input and ArrowDown on the last move the caret into
  the neighbouring input (composerCaretNav.ts), a click on the field's own
  surface puts the caret in the nearest input, and only the last input shows
  a placeholder, while nothing has been typed anywhere. Removing a quote
  moves focus that was in it to the input its text joined, and is a step on
  the browser's undo stack like the typing around it: Cmd+Z puts the quote
  back where it was, and redo takes it out again.
-->
<script lang="ts" module>
  // min-h instead of h so the vertical model/effort label can grow the button.
  const controlBaseClass =
    'min-h-9 gap-1.5 rounded-md border border-[var(--border-muted)] bg-[var(--bg-primary)] text-sm font-medium text-muted-foreground shadow-none transition-colors hover:bg-[var(--bg-hover)] hover:text-foreground mobile:min-h-11 mobile:justify-center';

  /** Styling for the agent config picker trigger rendered in the control row. */
  export const composerControlClass = `${controlBaseClass} px-4 py-1`;

  /** Attach button — same chrome as the config picker trigger. h-auto (not a
      fixed-height size variant) so the control row can stretch it to match
      the tallest control. */
  const attachButtonClass = `${composerControlClass} h-auto [&_svg]:!size-4`;

  /**
   * Puts a quote `onRemoveQuote` took out back where it was, reply and
   * lead-in text included, and answers whether it did: false once the draft
   * it was taken from is no longer the one in the composer. `then` runs when
   * the quote is in the composer again, which may be later than the call.
   */
  export type RestoreRemovedQuote = (then: () => void) => boolean;
</script>

<script lang="ts">
  import { onDestroy, tick, type Snippet } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import Paperclip from '@lucide/svelte/icons/paperclip';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import FileText from '@lucide/svelte/icons/file-text';
  import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
  import Spinner from '../../shared/Spinner.svelte';
  import { Button, buttonVariants } from '$lib/components/ui/button';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { cn } from '$lib/components/utils';
  import {
    COMPOSER_NEW_SESSION_OPTIONS,
    COMPOSER_SEND_MODE_OPTIONS,
    composerSendMenuTitle,
    composerSubmitBlock,
    composerSubmitBlockTitle,
    type ComposerNewSessionKind,
    type ComposerSendControl,
    type ComposerSendMode,
    type ComposerSessionBlock,
  } from './composerSendControl';
  import { ATTACHMENT_ACCEPT } from './attachErrors';
  import AttachmentThumb from './AttachmentThumb.svelte';
  import ComposerError from './ComposerError.svelte';
  import type { AttachmentPreview } from './attachmentPreview';
  import HashtagInput from './HashtagInput.svelte';
  import ComposerDraftQuotes from './ComposerDraftQuotes.svelte';
  import type { DraftQuote } from './composerDraft';
  import { focusAtEndSync, focusAtStartSync } from '../../shared/focusAtEnd';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import NativeUndoAnchor from '../../shared/NativeUndoAnchor.svelte';
  import { visibleViewport, watchVisualViewport } from '../../shared/visualViewport.svelte';
  import { adjacentEditor, caretLineEdges, isPlainVerticalArrow } from './composerCaretNav';
  import { revealElementInField, revealInField } from './composerFieldReveal';
  import { createCompositionSendGate, createTouchPressFocusGuard } from './composerSendPress';
  import type { HashtagItem } from '../../types';

  interface Props {
    /** Raw input text including #type:id tokens — the free text after any quotes. */
    value: string;
    /** The draft's quote blocks, in order, each with the free text before it. */
    quotes?: DraftQuote[];
    /**
     * Take a quote out of the draft. Returns how to put it back, which makes
     * the removal undoable; nothing when no quote was removed.
     */
    onRemoveQuote?: (id: string) => RestoreRemovedQuote | null | void;
    /** The contenteditable input element (for focus/resize from the parent). */
    textareaEl?: HTMLElement | null;
    /**
     * What the submit control shows (label, title, placeholder, whether it
     * is split and what its dropdown lists), as derived by the pane with
     * `composerSendControl`. The pane acts on the same object, so this
     * component never derives one of its own.
     */
    sendControl: ComposerSendControl;
    hashtagItems?: HashtagItem[];
    onkeydown?: (e: KeyboardEvent) => void;
    oninput?: (e: Event) => void;
    /** Whether image attachment is available in this context. */
    canAttachImages?: boolean;
    /** IDs of images currently attached to the pending message. */
    attachedImageIds?: string[];
    /** Image ID → preview for the attached images and PDFs (null: failed to load). */
    imagePreviews?: Map<string, AttachmentPreview | null>;
    onAttachFiles?: (files: File[]) => void;
    onRemoveImage?: (imageId: string) => void;
    /** Why the last attachment was rejected, shown until the next attempt. */
    attachError?: string | null;
    /** Session is running — attach/remove disabled. */
    isLive?: boolean;
    /** The remembered submit mode, checked in the dropdown's mode switch. */
    sendMode?: ComposerSendMode;
    /** The user picked a mode from the split button's menu. */
    onSendModeChange?: (mode: ComposerSendMode) => void;
    sending?: boolean;
    /**
     * Why there is no session row to submit to — still loading, or the load
     * settled without one — or `null` once the row is in. While set, submit
     * is disabled with a title saying which (and Cmd+Enter is the parent's
     * no-op) while the draft can still be typed.
     */
    sessionBlock?: ComposerSessionBlock | null;
    /**
     * Why the last composer action failed (send refused, queue change
     * failed), shown inline until the parent clears it — on the next
     * successful action or when the draft is edited.
     */
    sendError?: string | null;
    onSend?: () => void;
    /**
     * Start a new session of `kind` from the draft instead of sending it:
     * what the dropdown's new-session actions call. Whether they are listed
     * is `sendControl.newSessionMenu`.
     */
    onStartNewSession?: (kind: ComposerNewSessionKind) => void;
    /** The host cannot start a session right now: the actions show disabled. */
    newSessionDisabled?: boolean;
    /** Agent config picker rendered at the start of the control row. */
    configPicker?: Snippet;
  }

  let {
    value = $bindable(''),
    quotes = $bindable([]),
    onRemoveQuote,
    textareaEl = $bindable(null),
    sendControl,
    hashtagItems = [],
    onkeydown,
    oninput,
    canAttachImages = false,
    attachedImageIds = [],
    imagePreviews = new Map(),
    onAttachFiles,
    onRemoveImage,
    attachError = null,
    isLive = false,
    sendMode = 'queue',
    onSendModeChange,
    sending = false,
    sessionBlock = null,
    sendError = null,
    onSend,
    onStartNewSession,
    newSessionDisabled = false,
    configPicker,
  }: Props = $props();

  let fileInputEl = $state<HTMLInputElement>();
  let draftQuotesView = $state<ReturnType<typeof ComposerDraftQuotes>>();
  let undoAnchor = $state<ReturnType<typeof NativeUndoAnchor>>();
  let fieldEl = $state<HTMLDivElement>();
  /** A removal is being recorded for undo, which borrows focus and hands it back. */
  let recordingUndo = false;

  /** Of the screen left above the on-screen keyboard, how much the field may take. */
  const KEYBOARD_FIELD_SHARE = 0.4;

  // With the keyboard up the field is capped to a share of what is left on
  // screen (its height is known only then; otherwise the stylesheet's cap
  // stands), so the controls under it stay in view. Subscribed from here, so
  // a composer outside a full-screen dialog is covered too.
  $effect(() => watchVisualViewport());
  let fieldCap = $derived(
    visibleViewport.height !== null
      ? `${Math.round(visibleViewport.height * KEYBOARD_FIELD_SHARE)}px`
      : null
  );

  /** Something has been typed somewhere in the draft, so the field's one placeholder is hidden. */
  let hasDraftText = $derived(
    value !== '' || quotes.some((quote) => quote.before !== '' || quote.reply !== '')
  );

  /**
   * Scroll the field so the input being typed in is in view: on focus, as
   * the cap moves under it, and after each caret placed by hand below. Only
   * the field scrolls; the pane and the page around it stay where they are,
   * which is also why every focus call here passes `preventScroll` — the
   * browser's own scroll-on-focus would take the dialog and the page along
   * to an input below the fold.
   *
   * On `focusin` the caret isn't placed yet (the helpers focus first and
   * set the range after), so for an input taller than the field's view —
   * where only the caret's line can say where to scroll — the handlers
   * call this again once the range is in.
   *
   * Focus coming back from recording a removal for undo is not a move: the
   * field stays scrolled where the user had it.
   */
  function revealFocused() {
    if (recordingUndo) return;
    const active = document.activeElement;
    if (!fieldEl || !(active instanceof HTMLElement) || !fieldEl.contains(active)) return;
    revealInField(fieldEl, active);
  }

  const noScroll = { preventScroll: true } as const;

  /** Focus `editor` with the caret at one end, then bring that caret into the field's view. */
  function placeCaretIn(editor: HTMLElement | null, caret: 'start' | 'end') {
    if (caret === 'start') focusAtStartSync(editor, noScroll);
    else focusAtEndSync(editor, noScroll);
    revealFocused();
  }

  $effect(() => {
    void fieldCap;
    revealFocused();
  });

  /** The field's inputs — gaps, replies and the main input — in document order. */
  function fieldEditors(): HTMLElement[] {
    return fieldEl ? [...fieldEl.querySelectorAll<HTMLElement>('.hashtag-editor')] : [];
  }

  /**
   * A press on the field's own surface — its box outside any input or block,
   * or the quote list's — puts the caret in the input nearest the pointer,
   * at the end nearest it, as a click in a textarea's margin does. It is
   * done on mousedown, with the default prevented: the surface can't take
   * focus, so letting the press through would blur the input to the page
   * until the click — dropping `:focus-within` for a flicker of the border,
   * closing an open hashtag dropdown, and on iOS arming the visual-viewport
   * re-measure — only for the click to focus an input again. A press on an
   * input, a block or a button means those; a Shift-press or a secondary
   * button is left alone. A drag that starts on the surface selects nothing,
   * as one in a textarea's margin doesn't.
   */
  function handleFieldMousedown(event: MouseEvent) {
    if (event.button !== 0 || event.shiftKey) return;
    const target = event.target;
    if (!(target instanceof HTMLElement)) return;
    if (target !== fieldEl && !target.classList.contains('composer-draft-quotes')) return;
    event.preventDefault();
    let nearest: { editor: HTMLElement; distance: number; below: boolean } | null = null;
    for (const editor of fieldEditors()) {
      const box = editor.getBoundingClientRect();
      const below = event.clientY >= box.bottom;
      const distance =
        event.clientY < box.top ? box.top - event.clientY : below ? event.clientY - box.bottom : 0;
      // Ties go to the later input: the main input, for a click under everything.
      if (!nearest || distance <= nearest.distance) nearest = { editor, distance, below };
    }
    if (!nearest) return;
    placeCaretIn(nearest.editor, nearest.below ? 'end' : 'start');
  }

  /**
   * ArrowUp on the first line of an input moves the caret to the end of the
   * input before it; ArrowDown on the last line, to the start of the one
   * after, so the segments walk like the lines of one editor. Anything else
   * — a modifier, an IME candidate, the hashtag dropdown's own arrows (which
   * arrive prevented), a caret in the middle — is left to the input.
   */
  function handleFieldKeydown(event: KeyboardEvent) {
    if (event.defaultPrevented || !isPlainVerticalArrow(event)) return;
    if (anyComposing || isImeKeydown(event)) return;
    const target = event.target;
    if (!(target instanceof HTMLElement) || !target.classList.contains('hashtag-editor')) return;
    const editors = fieldEditors();
    if (!editors.includes(target)) return;
    const edges = caretLineEdges(target);
    if (!edges) return;
    const direction = event.key === 'ArrowUp' ? 'up' : 'down';
    if (direction === 'up' ? !edges.first : !edges.last) return;
    const next = adjacentEditor(editors, target, direction);
    if (!next) return;
    event.preventDefault();
    placeCaretIn(next, direction === 'up' ? 'end' : 'start');
  }

  /** Which input of a quote's block held the caret when the quote was removed, if one did. */
  type HeldCaret = 'gap' | 'reply' | null;

  /**
   * Take a quote out of the draft (the parent merges the text around it).
   * Focus in the removed block — its X, its reply, or the gap before it —
   * would fall to the page, so it moves to the input the gap's text joined:
   * the next quote's gap, or the main input after the last quote. The caret
   * goes where that text now ends, or to the start when there was none.
   * Returns the parent's way to put the quote back with where the caret was
   * in its block, or null when nothing was removed or it can't be put back.
   */
  function takeQuote(id: string): { restore: RestoreRemovedQuote; held: HeldCaret } | null {
    const index = quotes.findIndex((quote) => quote.id === id);
    if (index === -1) return null;
    const hadText = quotes[index].before.trim().length > 0;
    const nextId = quotes[index + 1]?.id ?? null;
    const hadFocus = draftQuotesView?.holdsFocus(id) ?? false;
    const inGap = document.activeElement?.classList.contains('composer-gap-input') ?? false;
    const held: HeldCaret = !hadFocus ? null : inGap ? 'gap' : 'reply';
    const restore = onRemoveQuote?.(id);
    if (hadFocus) {
      void tick().then(() => {
        const caret = hadText ? 'end' : 'start';
        if (nextId !== null && draftQuotesView?.focusGap(nextId, caret)) revealFocused();
        else placeCaretIn(textareaEl, caret);
      });
    }
    return restore ? { restore, held } : null;
  }

  /**
   * Remove a quote by hand — its X, or Backspace in the empty gap above it —
   * as a step on the browser's undo stack (see NativeUndoAnchor.svelte), in
   * order with the typing before and after it. Undo puts the quote back in
   * its place and redo takes it out again; neither does anything once the
   * draft the quote came from has been sent or switched away from.
   */
  function removeQuote(id: string) {
    let taken = takeQuote(id);
    if (!taken) return;
    recordingUndo = true;
    try {
      undoAnchor?.record({
        undo() {
          const held = taken?.held ?? null;
          const shown = () => void tick().then(() => showRestoredQuote(id, held));
          if (taken?.restore(shown)) taken = null;
        },
        redo() {
          // Only a quote undo put back: one a refused undo left out is still
          // out, and still this step's to restore.
          if (taken === null) taken = takeQuote(id);
        },
      });
    } finally {
      recordingUndo = false;
    }
  }

  /**
   * A quote undo put back, once it is rendered. The caret returns to the
   * input of its block it was taken from, as an undo returns it to the text
   * it restores; a quote removed with the caret elsewhere leaves it there,
   * as does an undo made with focus outside the field, and the field is
   * scrolled to show the block instead.
   */
  function showRestoredQuote(id: string, held: HeldCaret) {
    if (held !== null && fieldEl?.contains(document.activeElement)) {
      const placed =
        held === 'gap' ? draftQuotesView?.focusGap(id, 'end') : draftQuotesView?.focusReply(id);
      if (placed) {
        revealFocused();
        return;
      }
    }
    revealQuote(id);
  }

  /** Put the caret at the end of a quote block's reply; false if the block isn't shown. */
  export function focusQuoteReply(id: string): boolean {
    const focused = draftQuotesView?.focusReply(id) ?? false;
    if (focused) revealFocused();
    return focused;
  }

  /** Scroll the field so a quote block shows, without touching focus; false if the block isn't shown. */
  function revealQuote(id: string): boolean {
    const block = draftQuotesView?.blockFor(id) ?? null;
    if (!block || !fieldEl) return false;
    revealElementInField(fieldEl, block);
    return true;
  }

  let submitBlock = $derived(
    composerSubmitBlock({
      sessionBlock,
      sending,
      hasText: value.trim().length > 0 || quotes.length > 0,
    })
  );
  let sendDisabled = $derived(submitBlock !== null);
  let sendTitle = $derived(composerSubmitBlockTitle(submitBlock) ?? sendControl.title);
  let sendMenuTitle = $derived(composerSendMenuTitle(sendControl.newSessionMenu));

  function handleSendModeChange(value: string) {
    if (value === 'queue' || value === 'send') onSendModeChange?.(value);
  }

  // A touch press on Send leaves focus, and so the on-screen keyboard, in the
  // message box, as in a messaging app: the next message can be typed straight
  // away, and the full-screen dialog doesn't start resizing for a closing
  // keyboard under the tap. Only the press's compatibility mousedown is
  // cancelled (see createTouchPressFocusGuard); the click still fires. Mouse
  // presses keep the default, and so does the split button's menu trigger,
  // which moves focus into its menu as usual.
  //
  // While the box — or a quote's reply, or a gap between quotes — holds an
  // IME or dictation composition the press moves focus after all, which
  // commits the composition, and the send waits for its `compositionend`
  // (createCompositionSendGate) so the committed text is in the draft it
  // sends rather than landing in an input after it was cleared.
  // On Android, Gboard keeps the word being typed open as a composition, so
  // a Send there usually lets the keyboard drop: by design, rather than
  // committing the composition by hand; the message is still sent once.
  let composing = $state(false);
  /** A composition open in a gap or a quote's reply instead (see ComposerDraftQuotes). */
  let quotesComposing = $state(false);
  let anyComposing = $derived(composing || (quotes.length > 0 && quotesComposing));
  // The quote list unmounting mid-composition (the draft sent or replaced)
  // reports no end; a list mounted later must not inherit the flag.
  $effect(() => {
    if (quotes.length === 0) quotesComposing = false;
  });
  const sendPress = createTouchPressFocusGuard();
  const sendGate = createCompositionSendGate();
  let deferredSendTimer: ReturnType<typeof setTimeout> | undefined;

  function handleSendMouseDown(e: MouseEvent) {
    if (sendPress.mouseDown(anyComposing)) e.preventDefault();
  }

  function handleSendClick() {
    sendPress.reset();
    if (!sendGate.press(anyComposing)) return;
    // Sending now: a send still deferred from an earlier press must not
    // follow it.
    clearTimeout(deferredSendTimer);
    onSend?.();
  }

  function handleCompositionEnd() {
    if (!sendGate.compositionEnded()) return;
    // WebKit fires the committing `input` after `compositionend`; send once
    // it has reached the draft.
    clearTimeout(deferredSendTimer);
    deferredSendTimer = setTimeout(() => onSend?.(), 0);
  }

  // A composition the box dropped without a `compositionend` (its content
  // replaced, e.g. a failed send putting the draft back) drops a press
  // waiting on it too, so a later composition's end doesn't send. A real end
  // has already consulted the gate, synchronously, by the time this runs.
  $effect(() => {
    if (!anyComposing) sendGate.cancel();
  });

  onDestroy(() => clearTimeout(deferredSendTimer));

  function openImagePicker() {
    fileInputEl?.click();
  }

  function handleImageFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;
    onAttachFiles?.(Array.from(input.files));
    input.value = '';
  }
</script>

<div class="chat-composer">
  <NativeUndoAnchor bind:this={undoAnchor} />
  {#if canAttachImages && attachedImageIds.length > 0}
    <div class="reply-images">
      {#each attachedImageIds as imageId (imageId)}
        <div class="group/thumb reply-image-thumb">
          <AttachmentThumb preview={imagePreviews.get(imageId)} alt="attached" />
          {#if !isLive}
            <Button
              variant="ghost"
              size="icon"
              class="reply-image-remove-action absolute top-0.5 right-0.5 size-4 rounded-full bg-[var(--bg-deepest)] text-muted-foreground opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-chrome)] hover:text-foreground group-hover/thumb:opacity-100 [&_svg]:!size-2.5"
              title="Remove attachment"
              aria-label="Remove attachment"
              onclick={() => onRemoveImage?.(imageId)}
            >
              <X size={10} />
            </Button>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
  {#if canAttachImages}
    <ComposerError message={attachError} />
  {/if}
  <ComposerError message={sendError} />
  <!-- The field's press is a pointer convenience; its inputs are reached by Tab. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="composer-field"
    class:has-quotes={quotes.length > 0}
    class:has-draft-text={hasDraftText}
    bind:this={fieldEl}
    style:--composer-field-cap={fieldCap}
    onfocusin={revealFocused}
    onkeydown={handleFieldKeydown}
    onmousedown={handleFieldMousedown}
  >
    {#if quotes.length > 0}
      <ComposerDraftQuotes
        bind:this={draftQuotesView}
        bind:quotes
        bind:composing={quotesComposing}
        oncompositionend={handleCompositionEnd}
        onCaretPlaced={revealFocused}
        {hashtagItems}
        {onkeydown}
        {oninput}
        onRemoveQuote={removeQuote}
      />
    {/if}
    <HashtagInput
      bind:textareaEl
      bind:value
      bind:composing
      oncompositionend={handleCompositionEnd}
      class="message-input"
      placeholder={sendControl.placeholder}
      rows={1}
      {onkeydown}
      {oninput}
      items={hashtagItems}
    />
  </div>
  <div class="composer-controls">
    {@render configPicker?.()}
    {#if canAttachImages}
      <input
        bind:this={fileInputEl}
        type="file"
        accept="{ATTACHMENT_ACCEPT},image/svg+xml"
        multiple
        class="file-input-hidden"
        onchange={handleImageFileSelect}
      />
      <span class="inline-flex" title="Attach image">
        <Button
          variant="outline"
          class={attachButtonClass}
          aria-label="Attach image"
          onclick={openImagePicker}
          disabled={isLive}
        >
          <Paperclip size={16} />
          Attach
        </Button>
      </span>
    {/if}
    {#if sendControl.split}
      <span class="inline-flex composer-send composer-send-split touch-hit-group" title={sendTitle}>
        <Button
          variant="accent"
          class="composer-send-primary touch-hit h-auto min-h-9 gap-1.5 rounded-r-none px-4 py-1 text-sm mobile:min-h-11"
          onpointerdown={(e) => sendPress.pointerDown(e.pointerType)}
          onpointercancel={() => sendPress.reset()}
          onmousedown={handleSendMouseDown}
          onclick={handleSendClick}
          disabled={sendDisabled}
        >
          {@render sendLabel()}
        </Button>
        <DropdownMenu.Root>
          <DropdownMenu.Trigger
            class={cn(
              buttonVariants({ variant: 'accent' }),
              'composer-send-menu-trigger touch-hit -ml-px h-auto min-h-9 w-8 rounded-l-none px-0 mobile:min-h-11'
            )}
            title={sendMenuTitle}
            aria-label={sendMenuTitle}
            disabled={sending || sessionBlock !== null}
          >
            <ChevronDown size={14} />
          </DropdownMenu.Trigger>
          <DropdownMenu.Content align="end" side="top" sideOffset={4} class="min-w-[220px]">
            {#if sendControl.modeMenu}
              <DropdownMenu.RadioGroup value={sendMode} onValueChange={handleSendModeChange}>
                {#each COMPOSER_SEND_MODE_OPTIONS as option (option.mode)}
                  <DropdownMenu.RadioItem value={option.mode}>
                    <span class="flex min-w-0 flex-col">
                      <span>{option.label}</span>
                      <span class="truncate text-xs text-muted-foreground"
                        >{option.description}</span
                      >
                    </span>
                  </DropdownMenu.RadioItem>
                {/each}
              </DropdownMenu.RadioGroup>
            {/if}
            {#if sendControl.newSessionMenu}
              {#if sendControl.modeMenu}
                <DropdownMenu.Separator />
              {/if}
              {#each COMPOSER_NEW_SESSION_OPTIONS as option (option.kind)}
                <DropdownMenu.Item
                  disabled={sendDisabled || newSessionDisabled}
                  onSelect={() => onStartNewSession?.(option.kind)}
                >
                  {#if option.kind === 'note'}
                    <FileText size={14} class="text-[var(--note-color)]" />
                  {:else}
                    <GitCommitVertical size={14} class="text-[var(--commit-color)]" />
                  {/if}
                  <span class="flex min-w-0 flex-col">
                    <span>{option.label}</span>
                    <span class="truncate text-xs text-muted-foreground">{option.description}</span>
                  </span>
                </DropdownMenu.Item>
              {/each}
            {/if}
          </DropdownMenu.Content>
        </DropdownMenu.Root>
      </span>
    {:else}
      <span class="inline-flex composer-send" title={sendTitle}>
        <Button
          variant="accent"
          class="touch-hit h-auto min-h-9 gap-1.5 px-4 py-1 text-sm mobile:min-h-11"
          onpointerdown={(e) => sendPress.pointerDown(e.pointerType)}
          onpointercancel={() => sendPress.reset()}
          onmousedown={handleSendMouseDown}
          onclick={handleSendClick}
          disabled={sendDisabled}
        >
          {@render sendLabel()}
        </Button>
      </span>
    {/if}
  </div>
</div>

{#snippet sendLabel()}
  {#if sending}
    <Spinner size={14} />
    Sending…
  {:else}
    {sendControl.label}
  {/if}
{/snippet}

<style>
  /* The composer ends its dialog, so in a full-screen one its chrome runs
     under the home indicator and to the screen's sides, and only the
     controls are inset (`--dialog-safe-bottom`, `--dialog-safe-left` /
     `--dialog-safe-right`, see dialog-content). */
  .chat-composer {
    /* The undo anchor's containing block. */
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 14px;
    padding-bottom: calc(10px + var(--dialog-safe-bottom, 0px));
    padding-left: calc(14px + var(--dialog-safe-left, 0px));
    padding-right: calc(14px + var(--dialog-safe-right, 0px));
    border-top: 1px solid var(--border-subtle);
    background: var(--bg-chrome);
    flex-shrink: 0;
  }

  /* ----- Reply image previews -------------------------------------------- */

  .file-input-hidden {
    display: none;
  }

  .reply-images {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .reply-image-thumb {
    position: relative;
    width: 48px;
    height: 48px;
    border-radius: 6px;
    overflow: hidden;
    border: 1px solid var(--border-muted);
    background: var(--bg-hover);
    flex-shrink: 0;
  }

  .reply-image-thumb:focus-within :global(.reply-image-remove-action),
  :global(.reply-image-remove-action:focus-visible) {
    opacity: 1;
  }

  @media (hover: none), (pointer: coarse) {
    :global(.reply-image-remove-action) {
      opacity: 1;
    }
  }

  /* ----- The field ------------------------------------------------------- */

  /* One box for every input in the draft. It grows with its content to a cap
     — the main input's old 120px alone, more once quotes are in it — and
     then scrolls as one; with the on-screen keyboard up the cap is a share
     of the visible viewport instead (`--composer-field-cap`). The 1px border
     is the field's, so the inputs are measured 2px short of the heights the
     single input used to have. */
  .composer-field {
    --composer-field-max: 120px;
    display: flex;
    flex-direction: column;
    max-height: min(var(--composer-field-max), var(--composer-field-cap, 100vh));
    overflow-y: auto;
    overscroll-behavior: contain;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    cursor: text;
    transition: border-color 0.1s;
  }

  .composer-field.has-quotes {
    --composer-field-max: min(50vh, 480px);
  }

  .composer-field:focus-within {
    border-color: var(--border-emphasis);
  }

  /* The field's items — the quote list and the main input's wrapper — keep
     their content height; the field scrolls rather than squeezing them. */
  .composer-field > :global(*) {
    flex-shrink: 0;
  }

  .chat-composer :global(.message-input) {
    --hashtag-input-min-height: 34px;
    padding: 7px 12px;
    background: transparent;
    border: none;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
    resize: none;
  }

  /* After a quote block the main input is the text under it. */
  .composer-field.has-quotes :global(.message-input) {
    --hashtag-input-min-height: 30px;
    padding-top: 3px;
  }

  /* The one placeholder, in the last segment, only while nothing has been typed. */
  .composer-field.has-draft-text :global(.message-input.is-empty::before) {
    content: none;
  }

  @media (pointer: coarse) {
    .composer-field.has-quotes :global(.message-input) {
      --hashtag-input-min-height: 44px;
      padding: 12px;
    }
  }

  /* ----- Control row -------------------------------------------------------- */

  /* Stretch so every control matches the tallest one (the config picker can
     grow to two lines for its vertical model/effort label). */
  .composer-controls {
    display: flex;
    align-items: stretch;
    gap: 8px;
  }

  .composer-controls .composer-send {
    margin-left: auto;
  }

  /* Too little height for two rows (the keyboard up on a phone held
     sideways, dialog-content's `cramped`): the message box and Send share
     one, and the other controls wait for the keyboard to close. Anything
     else (attachments, an error line) keeps a full-width row of its own
     above it, a scroll up in SessionChatPane, which keeps the message box's
     row in view. */
  :global([data-cramped]) .chat-composer {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: end;
    padding-block: 5px;
  }

  :global([data-cramped]) .chat-composer > :global(*) {
    grid-column: 1 / -1;
  }

  :global([data-cramped]) .chat-composer > .composer-field {
    grid-column: 1;
  }

  :global([data-cramped]) .chat-composer > .composer-controls {
    grid-column: 2;
  }

  :global([data-cramped]) .composer-controls > :global(:not(.composer-send)) {
    display: none;
  }

  /* Send matches the message box's height, so the row fits the 48px the
     keyboard leaves (SessionChatPane keeps it in view); its touch area
     stays 44px (`touch-hit`). */
  :global([data-cramped]) .composer-send :global(button) {
    min-height: 36px;
  }

  /* A hairline between the split button's two accent segments, exactly the
     height of the fills. Every button paints a 1px transparent border with
     bg-clip-padding, so its fill is inset 1px from the border box on all
     sides. The trigger's -ml-px overlaps the primary's transparent right
     border column; with no left border of its own the trigger's fill paints
     over that column and the two fills meet with no seam. The divider is then
     drawn on top as a 1px ::before spanning the trigger's padding box (the
     containing block of an absolutely positioned child), which is the fill's
     extent, so it neither overshoots the fills nor notches them. A tinted
     border-left would instead span the border box and mitre into the
     transparent top and bottom borders, poking 1px past the fills at each
     end. The colour mixes --bg-primary into the accent so the line is an
     opaque tint of the fill rather than a wash over the chrome behind it. */
  .composer-send-split :global(.composer-send-menu-trigger) {
    position: relative;
    border-left-width: 0;
  }

  .composer-send-split :global(.composer-send-menu-trigger::before) {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 1px;
    background: color-mix(in srgb, var(--bg-primary) 35%, var(--ui-accent));
  }
</style>
