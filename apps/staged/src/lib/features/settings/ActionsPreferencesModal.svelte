<script lang="ts">
  import { X } from 'lucide-svelte';
  import { createBackdropDismissHandlers } from '../../shared/backdropDismiss';
  import ActionsSettingsPanel from './ActionsSettingsPanel.svelte';

  interface Props {
    onClose: () => void;
  }

  let { onClose }: Props = $props();

  const backdropDismiss = createBackdropDismissHandlers({
    onDismiss: () => onClose(),
  });
</script>

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
    <header class="modal-header">
      <h2>Actions Preferences</h2>
      <button class="close-btn" onclick={onClose}>
        <X size={16} />
      </button>
    </header>

    <div class="modal-body">
      <ActionsSettingsPanel />
    </div>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: var(--shadow-overlay);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1200;
  }

  .modal {
    width: min(1160px, 94vw);
    max-height: 88vh;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 14px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .modal-header h2 {
    margin: 0;
    font-size: var(--size-md);
    font-weight: 600;
  }

  .close-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
  }

  .close-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .modal-body {
    min-height: 0;
    flex: 1;
    padding: 14px;
    overflow: hidden;
  }

  @media (max-width: 640px) {
    .modal-backdrop {
      align-items: stretch;
      justify-content: stretch;
    }

    .modal {
      width: 100vw;
      max-height: none;
      height: 100vh;
      height: 100dvh;
      border-radius: 0;
      box-shadow: none;
    }

    .close-btn {
      width: 40px;
      height: 40px;
    }

    .modal-body {
      padding: 12px;
      overflow: auto;
    }
  }
</style>
