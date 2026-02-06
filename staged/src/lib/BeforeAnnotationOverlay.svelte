<!--
  BeforeAnnotationOverlay.svelte - AI annotation blur overlay for the "before" pane
  
  Renders a blurred overlay on the left (before) pane showing what the old code
  was doing. Uses the before_description field from annotations.
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

  // Use before_description if available, otherwise fall back to a generic message
  let displayText = $derived(annotation.before_description || 'Previous implementation');
</script>

<div
  class="before-annotation-overlay"
  class:revealed
  class:category-explanation={annotation.category === 'explanation'}
  class:category-warning={annotation.category === 'warning'}
  class:category-suggestion={annotation.category === 'suggestion'}
  class:category-context={annotation.category === 'context'}
  style="top: {top}px; height: {height}px; width: {containerWidth}px;"
>
  <p class="annotation-text">{displayText}</p>
</div>

<style>
  .before-annotation-overlay {
    position: absolute;
    left: 0;
    z-index: 10;

    /* Width is set via inline style from containerWidth prop */

    /* Blur effect on the code underneath */
    backdrop-filter: blur(6px);
    -webkit-backdrop-filter: blur(6px);

    /* Layout - right aligned for before pane (opposite of after pane) */
    display: flex;
    align-items: center;
    justify-content: flex-end;
    padding: 8px 16px 8px 12px; /* extra right padding for space before the accent bar */

    /* Transitions - default hidden, shown when holding A */
    opacity: 0;
    pointer-events: none;
    transition: opacity 200ms ease-out;

    /* Category accent on right edge (opposite of after pane) */
    border-right: 3px solid var(--annotation-accent);
  }

  .before-annotation-overlay.revealed {
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
    color: var(--text-secondary);
    font-style: italic;
    text-align: right;
    /* Wrap text within container */
    white-space: normal;
    word-wrap: break-word;
    overflow-wrap: break-word;
  }
</style>
