<!--
  ImageAttachment.svelte — Attach images or text snippets to a session prompt

  Provides a file picker button, thumbnail previews with remove buttons,
  and clipboard paste support (Ctrl/Cmd+V). Images are uploaded to the
  backend as base64-encoded data via the `create_image_from_data` command.

  Text snippets are modal-local (folded into the prompt on submit, never
  persisted): they render as chips alongside the image thumbnails, and an
  optional "Attach clipboard" button asks the parent to read the clipboard and
  attach it as a snippet.

  Props:
    branchId        — branch to associate images with, or null for project-only images
    projectId       — project to associate images with
    disabled        — disable interactions (e.g. while session is starting)
    imageIds        — current list of attached image IDs
    onImageIdsChange — callback when the image list changes
    textSnippets     — current list of attached text snippets
    onRemoveSnippet  — callback to remove a snippet by id
    onAttachClipboard — callback to attach the clipboard as a snippet; the
                        "Attach clipboard" button is shown only when supplied
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import ImagePlus from '@lucide/svelte/icons/image-plus';
  import Plus from '@lucide/svelte/icons/plus';
  import ClipboardPaste from '@lucide/svelte/icons/clipboard-paste';
  import FileText from '@lucide/svelte/icons/file-text';
  import { Button } from '$lib/components/ui/button';
  import { createImageFromData, getImageData, deleteImage } from '../../commands';
  import type { Image } from '../../types';
  import type { TextSnippet } from './sessionModalHelpers';

  type ImageIdsUpdate = string[] | ((current: string[]) => string[]);

  interface Props {
    branchId: string | null;
    projectId: string;
    disabled?: boolean;
    imageIds: string[];
    onImageIdsChange: (update: ImageIdsUpdate) => void;
    textSnippets?: TextSnippet[];
    onRemoveSnippet?: (id: string) => void;
    onAttachClipboard?: () => void;
  }

  let {
    branchId,
    projectId,
    disabled = false,
    imageIds,
    onImageIdsChange,
    textSnippets = [],
    onRemoveSnippet,
    onAttachClipboard,
  }: Props = $props();

  let hasAttachments = $derived(imageIds.length > 0 || textSnippets.length > 0);

  let previews = $state<Map<string, string>>(new Map());
  let fileInput: HTMLInputElement;

  // Load previews for existing images
  $effect(() => {
    for (const id of imageIds) {
      if (!previews.has(id)) {
        getImageData(id)
          .then((dataUrl) => {
            previews = new Map(previews);
            previews.set(id, dataUrl);
          })
          .catch((err) => {
            console.error('Failed to load image preview:', err);
            // Insert sentinel to prevent infinite retries
            previews = new Map(previews);
            previews.set(id, '');
          });
      }
    }
  });

  function openFilePicker() {
    fileInput?.click();
  }

  async function handleFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files) return;

    for (const file of Array.from(input.files)) {
      await addImageFile(file);
    }
    input.value = '';
  }

  async function addImageFile(file: File) {
    const buffer = await file.arrayBuffer();
    const bytes = new Uint8Array(buffer);
    // Convert to base64 using chunked approach to avoid O(n²) string concatenation
    const chunks = [];
    for (let i = 0; i < bytes.length; i += 8192) {
      chunks.push(String.fromCharCode(...bytes.subarray(i, i + 8192)));
    }
    const base64 = btoa(chunks.join(''));

    try {
      const image = await createImageFromData(
        branchId,
        projectId,
        file.name,
        file.type,
        base64,
        true
      );
      onImageIdsChange((current) => [...current, image.id]);
      // Set preview immediately from the local data
      const dataUrl = `data:${file.type};base64,${base64}`;
      previews = new Map(previews);
      previews.set(image.id, dataUrl);
    } catch (err) {
      console.error('Failed to attach image:', err);
    }
  }

  function handlePaste(e: ClipboardEvent) {
    if (e.defaultPrevented) return;
    const items = e.clipboardData?.items;
    if (!items || disabled) return;
    for (const item of Array.from(items)) {
      if (item.type.startsWith('image/')) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) void addImageFile(file);
      }
    }
  }

  function removeImage(imageId: string) {
    onImageIdsChange((current) => current.filter((id) => id !== imageId));
    previews = new Map(previews);
    previews.delete(imageId);
    deleteImage(imageId).catch((err) => {
      console.error('Failed to delete image from backend:', err);
    });
  }
</script>

<svelte:window onpaste={handlePaste} />

<input
  bind:this={fileInput}
  type="file"
  accept="image/png,image/jpeg,image/gif,image/webp"
  multiple
  class="file-input-hidden"
  onchange={handleFileSelect}
/>

{#if hasAttachments}
  <div class="attached-images">
    {#each imageIds as imageId}
      <div class="group/thumb image-thumb">
        {#if previews.get(imageId)}
          <img src={previews.get(imageId)} alt="attached" />
        {:else}
          <div class="image-placeholder"><ImagePlus size={16} /></div>
        {/if}
        {#if !disabled}
          <Button
            variant="ghost"
            size="icon"
            class="image-remove-action absolute top-0.5 right-0.5 size-4 rounded-full bg-[var(--bg-deepest)] text-muted-foreground opacity-0 shadow-none transition-opacity hover:bg-[var(--bg-chrome)] hover:text-foreground group-hover/thumb:opacity-100 [&_svg]:!size-2.5"
            title="Remove image"
            aria-label="Remove image"
            onclick={() => removeImage(imageId)}
          >
            <X size={10} />
          </Button>
        {/if}
      </div>
    {/each}
    {#each textSnippets as snippet (snippet.id)}
      <div class="snippet-chip" title={snippet.text}>
        <FileText size={12} class="shrink-0 text-[var(--text-faint)]" />
        <span class="snippet-chip-label">{snippet.label}</span>
        {#if !disabled}
          <Button
            variant="ghost"
            size="icon"
            class="size-4 shrink-0 rounded-full text-muted-foreground shadow-none hover:bg-[var(--bg-chrome)] hover:text-foreground [&_svg]:!size-2.5"
            title="Remove snippet"
            aria-label="Remove snippet"
            onclick={() => onRemoveSnippet?.(snippet.id)}
          >
            <X size={10} />
          </Button>
        {/if}
      </div>
    {/each}
    {#if !disabled}
      <Button
        variant="outline"
        size="icon"
        class="size-12 shrink-0 rounded-md border border-dashed border-[var(--border-muted)] bg-transparent text-[var(--text-faint)] shadow-none hover:border-[var(--border-emphasis)] hover:bg-transparent hover:text-muted-foreground [&_svg]:!size-4"
        title="Add image"
        aria-label="Add image"
        onclick={openFilePicker}
      >
        <Plus size={16} />
      </Button>
      {#if onAttachClipboard}
        <Button
          variant="outline"
          size="icon"
          class="size-12 shrink-0 rounded-md border border-dashed border-[var(--border-muted)] bg-transparent text-[var(--text-faint)] shadow-none hover:border-[var(--border-emphasis)] hover:bg-transparent hover:text-muted-foreground [&_svg]:!size-4"
          title="Attach clipboard"
          aria-label="Attach clipboard"
          onclick={() => onAttachClipboard?.()}
        >
          <ClipboardPaste size={16} />
        </Button>
      {/if}
    {/if}
  </div>
{:else if !disabled}
  <div class="attach-controls">
    <Button
      variant="outline"
      type="button"
      class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground shadow-none hover:text-foreground max-[768px]:h-11 max-[768px]:justify-center"
      onclick={openFilePicker}
    >
      <ImagePlus size={14} />
      <span>Attach images</span>
    </Button>
    {#if onAttachClipboard}
      <Button
        variant="outline"
        type="button"
        class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground shadow-none hover:text-foreground max-[768px]:h-11 max-[768px]:justify-center"
        title="Attach clipboard"
        onclick={() => onAttachClipboard?.()}
      >
        <ClipboardPaste size={14} />
        <span>Attach clipboard</span>
      </Button>
    {/if}
  </div>
{/if}

<style>
  .file-input-hidden {
    display: none;
  }

  .attached-images {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .image-thumb {
    position: relative;
    width: 48px;
    height: 48px;
    border-radius: 6px;
    overflow: hidden;
    border: 1px solid var(--border-muted);
    background: var(--bg-hover);
    flex-shrink: 0;
  }

  .image-thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .image-thumb:focus-within :global(.image-remove-action),
  :global(.image-remove-action:focus-visible) {
    opacity: 1;
  }

  @media (hover: none), (pointer: coarse) {
    :global(.image-remove-action) {
      opacity: 1;
    }
  }

  .image-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    color: var(--text-faint);
  }

  .attach-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .snippet-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 220px;
    height: 48px;
    padding: 0 6px 0 10px;
    border-radius: 6px;
    border: 1px solid var(--border-muted);
    background: var(--bg-hover);
    flex-shrink: 0;
  }

  .snippet-chip-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--size-sm);
    color: var(--text-primary);
  }
</style>
