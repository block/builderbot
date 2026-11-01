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
  dropdown switches it. While the session row is still loading the control
  is disabled (titled "Loading session…") but the draft can be typed.

  Failures of the composer's own actions (`sendError`: a refused send, a
  queue change that failed) are shown on a line inside the composer, next to
  the attachment error, rather than in the pane's transcript slot — the
  transcript stays readable behind them.
-->
<script lang="ts" module>
  // min-h instead of h so the vertical model/effort label can grow the button.
  const controlBaseClass =
    'min-h-9 gap-1.5 rounded-md border border-[var(--border-muted)] bg-[var(--bg-primary)] text-sm font-medium text-muted-foreground shadow-none transition-colors hover:bg-[var(--bg-hover)] hover:text-foreground max-[640px]:min-h-11 max-[640px]:justify-center';

  /** Styling for the agent config picker trigger rendered in the control row. */
  export const composerControlClass = `${controlBaseClass} px-4 py-1`;

  /** Attach button — same chrome as the config picker trigger. h-auto (not a
      fixed-height size variant) so the control row can stretch it to match
      the tallest control. */
  const attachButtonClass = `${composerControlClass} h-auto [&_svg]:!size-4`;
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import Paperclip from '@lucide/svelte/icons/paperclip';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import Spinner from '../../shared/Spinner.svelte';
  import { Button, buttonVariants } from '$lib/components/ui/button';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { cn } from '$lib/components/utils';
  import {
    COMPOSER_LOADING_TITLE,
    COMPOSER_SEND_MODE_OPTIONS,
    composerSendControl,
    composerSubmitBlock,
    type ComposerSendMode,
  } from './composerSendControl';
  import { ATTACHMENT_ACCEPT } from './attachErrors';
  import AttachmentThumb from './AttachmentThumb.svelte';
  import type { AttachmentPreview } from './attachmentPreview';
  import HashtagInput from './HashtagInput.svelte';
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
     * The session row has not loaded yet: submit is disabled (and Cmd+Enter
     * is the parent's no-op) while the draft can still be typed.
     */
    loading?: boolean;
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
    loading = false,
    sendError = null,
    onSend,
    configPicker,
  }: Props = $props();

  let fileInputEl = $state<HTMLInputElement>();

  let sendControl = $derived(composerSendControl({ isLive, canSendLive, mode: sendMode }));
  let submitBlock = $derived(
    composerSubmitBlock({ loading, sending, hasText: value.trim().length > 0 })
  );
  let sendDisabled = $derived(submitBlock !== null);
  let sendTitle = $derived(submitBlock === 'loading' ? COMPOSER_LOADING_TITLE : sendControl.title);

  function handleSendModeChange(value: string) {
    if (value === 'queue' || value === 'send') onSendModeChange?.(value);
  }

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
  {#if canAttachImages && attachError}
    <p class="composer-error" role="alert">{attachError}</p>
  {/if}
  {#if sendError}
    <p class="composer-error" role="alert">{sendError}</p>
  {/if}
  <HashtagInput
    bind:textareaEl
    bind:value
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
      <span class="inline-flex composer-send composer-send-split" title={sendTitle}>
        <Button
          variant="accent"
          class="composer-send-primary h-auto min-h-9 gap-1.5 rounded-r-none px-4 py-1 text-sm max-[640px]:min-h-11"
          onclick={onSend}
          disabled={sendDisabled}
        >
          {@render sendLabel()}
        </Button>
        <DropdownMenu.Root>
          <DropdownMenu.Trigger
            class={cn(
              buttonVariants({ variant: 'accent' }),
              'composer-send-menu-trigger h-auto min-h-9 w-8 rounded-l-none px-0 max-[640px]:min-h-11'
            )}
            title="Change how messages are delivered"
            aria-label="Change how messages are delivered"
            disabled={sending || loading}
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
          class="h-auto min-h-9 gap-1.5 px-4 py-1 text-sm max-[640px]:min-h-11"
          onclick={onSend}
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
  .chat-composer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px 14px;
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

  /* Attachment and send failures share one line style. */
  .composer-error {
    margin: 0;
    font-size: var(--size-sm);
    color: var(--ui-danger);
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

  /* A hairline between the split button's two accent segments. */
  .composer-send-split :global(.composer-send-menu-trigger) {
    border-left-color: color-mix(in srgb, var(--bg-primary) 35%, transparent);
  }
</style>
