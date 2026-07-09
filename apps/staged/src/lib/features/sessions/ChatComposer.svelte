<!--
  ChatComposer.svelte — Shared bottom chat input for session chat surfaces.

  Houses the attached-image strip, the text input, and the control row below
  it (agent config picker slot, attach button, send/stop button). Rendered by
  SessionChatPane, which backs both the session dialog and the note dialog's
  chat pane, so the composer looks and behaves identically everywhere: the
  text input gets its own full-width row with the controls beneath it.
-->
<script lang="ts" module>
  // min-h instead of h so the vertical model/effort label can grow the button.
  const controlBaseClass =
    'min-h-9 gap-1.5 rounded-md border border-[var(--border-muted)] bg-[var(--bg-primary)] text-sm font-medium text-muted-foreground shadow-none transition-colors hover:bg-[var(--bg-hover)] hover:text-foreground max-[640px]:min-h-11 max-[640px]:justify-center';

  /** Styling for the agent config picker trigger rendered in the control row. */
  export const composerControlClass = `${controlBaseClass} px-4 py-1`;

  /** Attach button — same chrome as the config picker trigger, icon-sized.
      h-auto (not the icon size variant) so the control row can stretch it to
      match the tallest control. */
  const attachButtonClass = `${controlBaseClass} h-auto w-9 px-0 justify-center [&_svg]:!size-4`;
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';
  import X from '@lucide/svelte/icons/x';
  import CircleStop from '@lucide/svelte/icons/circle-stop';
  import Paperclip from '@lucide/svelte/icons/paperclip';
  import ImagePlus from '@lucide/svelte/icons/image-plus';
  import Spinner from '../../shared/Spinner.svelte';
  import { Button } from '$lib/components/ui/button';
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
    /** Image ID → data URL previews for the attached images. */
    imagePreviews?: Map<string, string>;
    onAttachFiles?: (files: File[]) => void;
    onRemoveImage?: (imageId: string) => void;
    /** Session is running — attach/remove disabled, stop replaces send. */
    isLive?: boolean;
    sending?: boolean;
    cancelling?: boolean;
    onSend?: () => void;
    onStop?: () => void;
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
    isLive = false,
    sending = false,
    cancelling = false,
    onSend,
    onStop,
    configPicker,
  }: Props = $props();

  let fileInputEl = $state<HTMLInputElement>();

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
          {#if imagePreviews.get(imageId)}
            <img src={imagePreviews.get(imageId)} alt="attached" />
          {:else}
            <div class="reply-image-placeholder"><ImagePlus size={16} /></div>
          {/if}
          {#if !isLive}
            <Button
              variant="ghost"
              size="icon"
              class="reply-image-remove-action absolute top-0.5 right-0.5 size-4 rounded-full bg-[var(--bg-deepest)] text-muted-foreground opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-chrome)] hover:text-foreground group-hover/thumb:opacity-100 [&_svg]:!size-2.5"
              title="Remove image"
              aria-label="Remove image"
              onclick={() => onRemoveImage?.(imageId)}
            >
              <X size={10} />
            </Button>
          {/if}
        </div>
      {/each}
    </div>
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
        accept="image/png,image/jpeg,image/gif,image/webp,image/svg+xml"
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
        </Button>
      </span>
    {/if}
    {#if isLive}
      <span class="inline-flex composer-send" title="Stop session">
        <Button
          variant="destructive"
          class="h-auto min-h-9 w-9 shrink-0 rounded-[10px] px-0 [&_svg]:!size-4"
          aria-label="Stop session"
          onclick={onStop}
          disabled={cancelling}
        >
          {#if cancelling}
            <Spinner size={16} />
          {:else}
            <CircleStop size={16} />
          {/if}
        </Button>
      </span>
    {:else}
      <span class="inline-flex composer-send" title="Send message">
        <Button
          variant="accent"
          class="h-auto min-h-9 gap-1.5 px-4 py-1 text-sm max-[640px]:min-h-11"
          onclick={onSend}
          disabled={sending || !value.trim()}
        >
          {#if sending}
            <Spinner size={14} />
            Sending…
          {:else}
            Send
          {/if}
        </Button>
      </span>
    {/if}
  </div>
</div>

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

  .reply-image-thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .reply-image-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    color: var(--text-faint);
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
</style>
