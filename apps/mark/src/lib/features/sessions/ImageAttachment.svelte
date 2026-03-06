<!--
  ImageAttachment.svelte — Attach images to a session prompt

  Provides a file picker button, thumbnail previews with remove buttons,
  and clipboard paste support (Ctrl/Cmd+V). Images are uploaded to the
  backend as base64-encoded data via the `create_image_from_data` command.

  Props:
    branchId        — branch to associate images with
    projectId       — project to associate images with
    disabled        — disable interactions (e.g. while session is starting)
    imageIds        — current list of attached image IDs
    onImageIdsChange — callback when the list changes
-->
<script lang="ts">
  import { X, ImagePlus, Plus } from 'lucide-svelte';
  import { createImageFromData, getImageData, deleteImage } from '../../commands';
  import type { Image } from '../../types';

  interface Props {
    branchId: string;
    projectId: string;
    disabled?: boolean;
    imageIds: string[];
    onImageIdsChange: (ids: string[]) => void;
  }

  let { branchId, projectId, disabled = false, imageIds, onImageIdsChange }: Props = $props();

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
      onImageIdsChange([...imageIds, image.id]);
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
    onImageIdsChange(imageIds.filter((id) => id !== imageId));
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

{#if imageIds.length > 0}
  <div class="attached-images">
    {#each imageIds as imageId}
      <div class="image-thumb">
        {#if previews.get(imageId)}
          <img src={previews.get(imageId)} alt="attached" />
        {:else}
          <div class="image-placeholder"><ImagePlus size={16} /></div>
        {/if}
        {#if !disabled}
          <button class="remove-btn" onclick={() => removeImage(imageId)} title="Remove image">
            <X size={10} />
          </button>
        {/if}
      </div>
    {/each}
    {#if !disabled}
      <button class="add-more-btn" onclick={openFilePicker} title="Add image">
        <Plus size={16} />
      </button>
    {/if}
  </div>
{:else if !disabled}
  <button class="attach-btn" onclick={openFilePicker} type="button">
    <ImagePlus size={14} />
    <span>Attach images</span>
  </button>
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

  .image-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    color: var(--text-faint);
  }

  .remove-btn {
    position: absolute;
    top: 2px;
    right: 2px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    padding: 0;
    border: none;
    border-radius: 50%;
    background: var(--bg-deepest);
    color: var(--text-muted);
    cursor: pointer;
    opacity: 0;
    transition:
      opacity 0.1s,
      color 0.1s;
  }

  .image-thumb:hover .remove-btn {
    opacity: 1;
  }

  .remove-btn:hover {
    color: var(--text-primary);
    background: var(--bg-chrome);
  }

  .add-more-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    border-radius: 6px;
    border: 1px dashed var(--border-muted);
    background: none;
    color: var(--text-faint);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .add-more-btn:hover {
    color: var(--text-muted);
    border-color: var(--border-emphasis);
  }

  .attach-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-radius: 6px;
    border: 1px dashed var(--border-muted);
    background: none;
    color: var(--text-faint);
    font-size: var(--size-xs);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s;
  }

  .attach-btn:hover {
    color: var(--text-muted);
    border-color: var(--border-emphasis);
  }
</style>
