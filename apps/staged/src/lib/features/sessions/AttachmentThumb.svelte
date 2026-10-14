<!--
  AttachmentThumb.svelte — Contents of one chat attachment thumbnail slot

  Fills a positioned, sized parent (composer strip, chat history bubble,
  queued message) with the attachment's preview: the image itself, or the
  rendered first page of a PDF with a "PDF" badge. While the preview is
  loading, or when a PDF could not be rendered, an icon stands in.

  Props:
    preview  — backend thumbnail; undefined while loading, null when the
               attachment could not be loaded at all
    alt      — alt text for the image
    iconSize — size of the stand-in icon
-->
<script lang="ts">
  import ImagePlus from '@lucide/svelte/icons/image-plus';
  import FileText from '@lucide/svelte/icons/file-text';
  import { isPdfMimeType, type AttachmentPreview } from './attachmentPreview';

  interface Props {
    preview: AttachmentPreview | null | undefined;
    alt: string;
    iconSize?: number;
  }

  let { preview, alt, iconSize = 16 }: Props = $props();

  let isPdf = $derived(!!preview && isPdfMimeType(preview.mimeType));
  let title = $derived(preview?.filename || undefined);
</script>

{#if preview?.dataUrl}
  <img src={preview.dataUrl} {alt} {title} />
{:else}
  <div class="attachment-thumb-placeholder" {title}>
    {#if isPdf}
      <FileText size={iconSize} />
    {:else}
      <ImagePlus size={iconSize} />
    {/if}
  </div>
{/if}
{#if isPdf}
  <span class="attachment-thumb-badge" aria-hidden="true">PDF</span>
{/if}

<style>
  img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .attachment-thumb-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
    color: var(--text-faint);
  }

  .attachment-thumb-badge {
    position: absolute;
    left: 0;
    bottom: 0;
    padding: 0 3px;
    border-top-right-radius: 4px;
    background: var(--bg-deepest);
    color: var(--text-primary);
    font-size: 9px;
    font-weight: 600;
    line-height: 12px;
    letter-spacing: 0.02em;
    pointer-events: none;
  }
</style>
