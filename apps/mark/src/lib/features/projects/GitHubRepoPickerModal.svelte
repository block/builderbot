<!--
  GitHubRepoPickerModal.svelte - Overlay modal wrapper for the repo picker

  Thin wrapper that renders GitHubRepoPicker inside a modal backdrop.
  Used by ProjectHome for standalone "add repo" flows.
  NewProjectForm uses GitHubRepoPicker directly with an inline slide.
-->
<script lang="ts">
  import GitHubRepoPicker from './GitHubRepoPicker.svelte';

  interface Props {
    onSelect: (nameWithOwner: string, subpath?: string) => void;
    onClose: () => void;
    excludeRepos?: Set<string>;
  }

  let { onSelect, onClose, excludeRepos }: Props = $props();
  let backdropPointerDown = false;

  function handleBackdropPointerDown(event: PointerEvent) {
    backdropPointerDown = event.target === event.currentTarget;
  }

  function handleBackdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget && backdropPointerDown) {
      onClose();
    }
    backdropPointerDown = false;
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="modal-backdrop"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  onpointerdown={handleBackdropPointerDown}
  onclick={handleBackdropClick}
  onkeydown={(e) => e.key === 'Escape' && onClose()}
>
  <div class="modal">
    <GitHubRepoPicker {onSelect} onBack={onClose} {excludeRepos} />
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
    width: 520px;
    max-width: 90vw;
    max-height: 70vh;
    background-color: var(--bg-chrome);
    border-radius: 12px;
    box-shadow: var(--shadow-elevated);
    display: flex;
    flex-direction: column;
  }
</style>
