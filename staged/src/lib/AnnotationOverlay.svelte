<!--
  AnnotationOverlay.svelte - AI annotation blur overlay
  
  Renders a blurred overlay on annotated code regions with the AI commentary
  displayed on top. The overlay scrolls with the code and can be revealed
  by holding down the 'A' key.
-->
<script lang="ts">
  import type { SmartDiffAnnotation } from './types';

  interface Props {
    annotation: SmartDiffAnnotation;
    /** Top position in pixels (relative to lines-wrapper) */
    top: number;
    /** Height in pixels */
    height: number;
    /** Whether the annotation is currently revealed (overlay visible) */
    revealed: boolean;
    /** Width of the visible container in pixels */
    containerWidth: number;
  }

  let { annotation, top, height, revealed, containerWidth }: Props = $props();
</script>

<div
  class="annotation-overlay"
  class:revealed
  class:category-explanation={annotation.category === 'explanation'}
  class:category-warning={annotation.category === 'warning'}
  class:category-suggestion={annotation.category === 'suggestion'}
  class:category-context={annotation.category === 'context'}
  style="top: {top}px; height: {height}px; width: {containerWidth}px;"
>
  <p class="annotation-text">{annotation.content}</p>
</div>

<style>
  .annotation-overlay {
    position: absolute;
    left: 0;
    z-index: 10;

    /* Width is set via inline style from containerWidth prop */

    /* Blur effect on the code underneath */
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);

    /* Layout - left aligned */
    display: flex;
    align-items: center;
    justify-content: flex-start;
    padding: 8px 12px 8px 16px; /* extra left padding for space after the accent bar */

    /* Transitions - default hidden, shown when holding A */
    opacity: 0;
    pointer-events: none;
    transition: opacity 200ms ease-out;

    /* Category accent on left edge */
    border-left: 3px solid var(--annotation-accent);
  }

  .annotation-overlay.revealed {
    opacity: 1;
    pointer-events: auto;
  }

  /* Category colors */
  .category-explanation {
    --annotation-accent: var(--text-accent);
  }

  .category-warning {
    --annotation-accent: var(--status-modified);
  }

  .category-suggestion {
    --annotation-accent: var(--status-added);
  }

  .category-context {
    --annotation-accent: var(--text-muted);
  }

  .annotation-text {
    margin: 0;
    font-size: var(--size-sm);
    line-height: 1.5;
    color: var(--text-primary);
    text-align: left;
    /* Wrap text within container */
    white-space: normal;
    word-wrap: break-word;
    overflow-wrap: break-word;
  }
</style>
