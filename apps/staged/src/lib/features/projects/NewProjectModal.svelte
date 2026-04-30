<!--
  NewProjectModal.svelte - Modal wrapper for project creation form

  Renders the NewProjectForm inside a dialog overlay with header and close button.
-->
<script lang="ts">
  import { X } from 'lucide-svelte';
  import type { Project } from '../../types';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';
  import NewProjectForm from './NewProjectForm.svelte';

  interface Props {
    onCreated: (project: Project) => void;
    onClose: () => void;
  }

  let { onCreated, onClose }: Props = $props();
  const backdropDismiss = createBackdropDismissHandlers({ onDismiss: () => onClose() });

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onpointerdown={backdropDismiss.handlePointerDown}
  onclick={backdropDismiss.handleClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <div class="modal-header">
      <h2>New Project</h2>
      <button class="close-button" onclick={onClose}>
        <X size={18} />
      </button>
    </div>

    <div class="modal-body">
      <NewProjectForm {onCreated} onCancel={onClose} />
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background-color: var(--shadow-overlay);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: 1000;
  }

  .modal {
    width: 460px;
    max-width: 90vw;
    background-color: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-bottom: none;
    border-radius: 12px 12px 0 0;
  }

  .modal-header h2 {
    flex: 1;
    margin: 0;
    font-size: var(--size-md);
    font-weight: 500;
    color: var(--text-primary);
  }

  .close-button {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .close-button:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-body {
    padding: 16px;
  }

  @media (max-width: 640px) {
    .modal-backdrop {
      align-items: stretch;
      padding-top: 0;
    }

    .modal {
      width: 100vw;
      max-width: none;
      height: 100vh;
      height: 100dvh;
      border-radius: 0;
      box-shadow: none;
    }

    .modal-header {
      flex-shrink: 0;
      padding: 12px 16px;
      border-bottom: 1px solid var(--border-subtle);
      border-radius: 0;
    }

    .close-button {
      width: 40px;
      height: 40px;
    }

    .modal-body {
      flex: 1;
      min-height: 0;
      overflow: auto;
      padding: 16px;
    }
  }
</style>
