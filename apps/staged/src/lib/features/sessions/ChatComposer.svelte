<!--
  ChatComposer.svelte — Shared bottom chat input for session chat surfaces.

  Houses the attached-image strip, the text input, and the control row below
  it (agent config picker slot, attach button, send/queue button). Rendered by
  SessionChatPane, which backs both the session dialog and the note dialog's
  chat pane, so the composer looks and behaves identically everywhere: the
  text input gets its own full-width row with the controls beneath it.

  The submit control follows `composerSendControl`: a plain "Send" on an idle
  session, a plain "Queue" on a running one — and, when the backend reports
  the running session can take a message now (`canSendLive`), a split button
  whose primary segment performs the remembered mode (Queue / Send) and whose
  dropdown switches it. While the pane has no session row to submit to
  (`sessionBlock`: still loading, or the load settled without one) the
  control is disabled with a title saying which, but the draft can be typed.

  Failures of the composer's own actions (`sendError`: a refused send, a
  queue change that failed) are shown on a `ComposerError` line inside the
  composer, next to the attachment error, rather than in the pane's
  transcript slot — the transcript stays readable behind them.
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
</script>

<script lang="ts">
  import { onDestroy, type Snippet } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import Paperclip from '@lucide/svelte/icons/paperclip';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Spinner from '../../shared/Spinner.svelte';
  import { Button, buttonVariants } from '$lib/components/ui/button';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { cn } from '$lib/components/utils';
  import {
    COMPOSER_SEND_MODE_OPTIONS,
    composerSendControl,
    composerSubmitBlock,
    composerSubmitBlockTitle,
    type ComposerSendMode,
    type ComposerSessionBlock,
  } from './composerSendControl';
  import { ATTACHMENT_ACCEPT } from './attachErrors';
  import AttachmentThumb from './AttachmentThumb.svelte';
  import ComposerError from './ComposerError.svelte';
  import type { AttachmentPreview } from './attachmentPreview';
  import HashtagInput from './HashtagInput.svelte';
  import { createCompositionSendGate, createTouchPressFocusGuard } from './composerSendPress';
  import type { HashtagItem } from '../../types';

  interface Props {
    /** Raw input text including #type:id tokens. */
    value: string;
    /** The contenteditable input element (for focus/resize from the parent). */
    textareaEl?: HTMLElement | null;
    placeholder?: string;
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
    /** Session is running — attach/remove disabled, send queues instead. */
    isLive?: boolean;
    /**
     * The running session can take a message now (backend-reported), so the
     * submit control offers "Send" beside "Queue". Ignored unless `isLive`.
     */
    canSendLive?: boolean;
    /** The remembered submit mode for live-capable sessions. */
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
    /** Agent config picker rendered at the start of the control row. */
    configPicker?: Snippet;
  }

  let {
    value = $bindable(''),
    textareaEl = $bindable(null),
    placeholder = '',
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
    canSendLive = false,
    sendMode = 'queue',
    onSendModeChange,
    sending = false,
    sessionBlock = null,
    sendError = null,
    onSend,
    configPicker,
  }: Props = $props();

  let fileInputEl = $state<HTMLInputElement>();

  let sendControl = $derived(composerSendControl({ isLive, canSendLive, mode: sendMode }));
  let submitBlock = $derived(
    composerSubmitBlock({ sessionBlock, sending, hasText: value.trim().length > 0 })
  );
  let sendDisabled = $derived(submitBlock !== null);
  let sendTitle = $derived(composerSubmitBlockTitle(submitBlock) ?? sendControl.title);

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
  // While the box holds an IME or dictation composition the press moves focus
  // after all, which commits the composition, and the send waits for its
  // `compositionend` (createCompositionSendGate) so the committed text is in
  // the draft it sends rather than landing in the box after it was cleared.
  // On Android, Gboard keeps the word being typed open as a composition, so
  // a Send there usually lets the keyboard drop: by design, rather than
  // committing the composition by hand; the message is still sent once.
  let composing = $state(false);
  const sendPress = createTouchPressFocusGuard();
  const sendGate = createCompositionSendGate();
  let deferredSendTimer: ReturnType<typeof setTimeout> | undefined;

  function handleSendMouseDown(e: MouseEvent) {
    if (sendPress.mouseDown(composing)) e.preventDefault();
  }

  function handleSendClick() {
    sendPress.reset();
    if (!sendGate.press(composing)) return;
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
    if (!composing) sendGate.cancel();
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
  <HashtagInput
    bind:textareaEl
    bind:value
    bind:composing
    oncompositionend={handleCompositionEnd}
    class="message-input"
    {placeholder}
    rows={1}
    {onkeydown}
    {oninput}
    items={hashtagItems}
  />
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
            title="Change how messages are delivered"
            aria-label="Change how messages are delivered"
            disabled={sending || sessionBlock !== null}
          >
            <ChevronDown size={14} />
          </DropdownMenu.Trigger>
          <DropdownMenu.Content align="end" side="top" sideOffset={4} class="min-w-[220px]">
            <DropdownMenu.RadioGroup value={sendMode} onValueChange={handleSendModeChange}>
              {#each COMPOSER_SEND_MODE_OPTIONS as option (option.mode)}
                <DropdownMenu.RadioItem value={option.mode}>
                  <span class="flex min-w-0 flex-col">
                    <span>{option.label}</span>
                    <span class="truncate text-xs text-muted-foreground">{option.description}</span>
                  </span>
                </DropdownMenu.RadioItem>
              {/each}
            </DropdownMenu.RadioGroup>
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

  /* ----- Text input -------------------------------------------------------- */

  .chat-composer :global(.message-input) {
    padding: 7px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 10px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
    resize: none;
    overflow-y: hidden;
    min-height: 36px;
    max-height: 120px;
  }

  .chat-composer :global(.message-input):focus {
    outline: none;
    border-color: var(--border-emphasis);
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
     else (attachments, an error line) keeps a full-width row of its own and
     the dialog scrolls to it. */
  :global([data-cramped]) .chat-composer {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    align-items: end;
    padding-block: 5px;
  }

  :global([data-cramped]) .chat-composer > :global(*) {
    grid-column: 1 / -1;
  }

  :global([data-cramped]) .chat-composer > :global(.hashtag-input-wrapper) {
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
