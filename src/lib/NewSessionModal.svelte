<!--
  NewSessionModal.svelte - Start a new agent session on a branch

  Simple modal with a prompt input. The session will run in the branch's
  worktree and produce a commit when complete.

  The backend handles all context gathering (commits, notes, etc.) and
  builds the full prompt with timeline context.
-->
<script lang="ts">
  import {
    X,
    GitCommitHorizontal,
    GitBranch,
    Loader2,
    Send,
    Image as ImageIcon,
  } from 'lucide-svelte';
  import type { Branch } from './services/branch';
  import type { ImageAttachment } from './types';
  import { startBranchSession } from './services/branch';
  import AgentSelector from './AgentSelector.svelte';
  import type { AcpProvider } from './stores/agent.svelte';
  import { preferences } from './stores/preferences.svelte';

  interface Props {
    branch: Branch;
    onClose: () => void;
    onSessionStarted?: (branchSessionId: string, aiSessionId: string) => void;
    initialPrompt?: string;
  }

  let { branch, onClose, onSessionStarted, initialPrompt }: Props = $props();

  // State
  let prompt = $state('');

  // Initialize prompt from prop if provided
  $effect(() => {
    if (initialPrompt) {
      prompt = initialPrompt;
    }
  });
  let starting = $state(false);
  let error = $state<string | null>(null);
  let selectedProvider = $state<AcpProvider>((preferences.aiAgent as AcpProvider) || 'goose');
  let images = $state<ImageAttachment[]>([]);

  let textareaEl: HTMLTextAreaElement | null = $state(null);
  let fileInputEl: HTMLInputElement | null = $state(null);

  const MAX_FILE_SIZE = 10 * 1024 * 1024;
  const MAX_IMAGE_COUNT = 5;
  const ALLOWED_MIME_TYPES = ['image/png', 'image/jpeg', 'image/jpg', 'image/gif', 'image/webp'];

  // Focus textarea on mount
  $effect(() => {
    if (textareaEl) {
      textareaEl.focus();
    }
  });

  // Extract repo name from path
  function repoName(path: string): string {
    const parts = path.split('/');
    return parts[parts.length - 1] || path;
  }

  async function handleStart(e: Event) {
    e.preventDefault();
    if (!prompt.trim()) return;

    starting = true;
    error = null;

    try {
      const userPrompt = prompt.trim();

      // Backend handles all context gathering and prompt building
      const result = await startBranchSession(
        branch.id,
        userPrompt,
        selectedProvider,
        images.length > 0 ? images : undefined
      );

      // Notify parent that session started
      onSessionStarted?.(result.branchSessionId, result.aiSessionId);
      onClose();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      starting = false;
    }
  }

  async function handleFileSelect(e: Event) {
    const input = e.target as HTMLInputElement;
    if (!input.files || input.files.length === 0) return;

    try {
      if (images.length + input.files.length > MAX_IMAGE_COUNT) {
        error = `Too many images. Maximum ${MAX_IMAGE_COUNT} images allowed.`;
        if (input) input.value = '';
        return;
      }

      const newImages: ImageAttachment[] = [];
      for (let i = 0; i < input.files.length; i++) {
        const file = input.files[i];

        if (!file.type.startsWith('image/')) {
          error = `File ${file.name} is not an image`;
          continue;
        }

        if (!ALLOWED_MIME_TYPES.includes(file.type)) {
          error = `File ${file.name} has unsupported format. Allowed: PNG, JPEG, GIF, WebP`;
          continue;
        }

        if (file.size > MAX_FILE_SIZE) {
          error = `File ${file.name} is too large (max 10MB)`;
          continue;
        }

        const base64 = await fileToBase64(file);
        newImages.push({
          data: base64,
          mime_type: file.type,
        });
      }

      if (newImages.length > 0) {
        images = [...images, ...newImages];
        error = null;
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }

    if (input) input.value = '';
  }

  function fileToBase64(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const result = reader.result as string;
        const base64 = result.split(',')[1];
        resolve(base64);
      };
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
  }

  function removeImage(index: number) {
    images = images.filter((_, i) => i !== index);
  }

  function triggerFileInput() {
    fileInputEl?.click();
  }

  async function handlePaste(e: ClipboardEvent) {
    if (!e.clipboardData) return;

    const items = e.clipboardData.items;
    const imageFiles: File[] = [];

    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.type.startsWith('image/')) {
        const file = item.getAsFile();
        if (file) {
          imageFiles.push(file);
        }
      }
    }

    if (imageFiles.length === 0) return;

    e.preventDefault();

    try {
      if (images.length + imageFiles.length > MAX_IMAGE_COUNT) {
        error = `Too many images. Maximum ${MAX_IMAGE_COUNT} images allowed.`;
        return;
      }

      const newImages: ImageAttachment[] = [];
      for (const file of imageFiles) {
        if (!ALLOWED_MIME_TYPES.includes(file.type)) {
          error = `Pasted image has unsupported format. Allowed: PNG, JPEG, GIF, WebP`;
          continue;
        }

        if (file.size > MAX_FILE_SIZE) {
          error = `Pasted image is too large (max 10MB)`;
          continue;
        }

        const base64 = await fileToBase64(file);
        newImages.push({
          data: base64,
          mime_type: file.type,
        });
      }

      if (newImages.length > 0) {
        images = [...images, ...newImages];
        error = null;
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
      return;
    }

    // Cmd+Enter to submit
    if (e.key === 'Enter' && e.metaKey && prompt.trim() && !starting) {
      e.preventDefault();
      handleStart(e);
      return;
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
<div class="modal-backdrop" role="presentation" onclick={onClose}>
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div
    class="modal"
    role="dialog"
    aria-modal="true"
    tabindex="-1"
    onclick={(e) => e.stopPropagation()}
  >
    <header class="modal-header">
      <div class="header-content">
        <GitCommitHorizontal size={18} />
        <span class="header-title">New Commit</span>
      </div>
      <button class="close-btn" onclick={onClose}>
        <X size={18} />
      </button>
    </header>

    <form class="modal-content" onsubmit={handleStart}>
      <div class="branch-info">
        <GitBranch size={16} />
        <span class="branch-name">{branch.branchName}</span>
        <span class="repo-name">in {repoName(branch.repoPath)}</span>
      </div>

      <div class="form-group">
        <label for="prompt">What would you like to work on?</label>
        <textarea
          bind:this={textareaEl}
          bind:value={prompt}
          id="prompt"
          placeholder="Describe the task..."
          rows={4}
          disabled={starting}
          onpaste={handlePaste}
        ></textarea>
        <div class="prompt-actions">
          <button
            class="attach-button"
            type="button"
            onclick={triggerFileInput}
            title="Attach images"
            disabled={images.length >= MAX_IMAGE_COUNT}
          >
            <ImageIcon size={16} />
            <span
              >Attach Images {images.length > 0
                ? `(${images.length}/${MAX_IMAGE_COUNT})`
                : ''}</span
            >
          </button>
          <p class="hint">Press ⌘Enter to start</p>
        </div>
        <input
          bind:this={fileInputEl}
          type="file"
          accept="image/*"
          multiple
          style="display: none"
          onchange={handleFileSelect}
        />
      </div>

      {#if images.length > 0}
        <div class="images-preview">
          {#each images as image, i}
            <div class="image-item">
              <img src={`data:${image.mime_type};base64,${image.data}`} alt="Attachment {i + 1}" />
              <button class="remove-image" onclick={() => removeImage(i)} title="Remove image">
                <X size={14} />
              </button>
            </div>
          {/each}
        </div>
      {/if}

      {#if error}
        <div class="error-message">{error}</div>
      {/if}

      <div class="form-actions">
        <AgentSelector bind:provider={selectedProvider} disabled={starting} />
        <div class="action-buttons">
          <button type="button" class="cancel-btn" onclick={onClose} disabled={starting}>
            Cancel
          </button>
          <button type="submit" class="submit-btn" disabled={starting || !prompt.trim()}>
            {#if starting}
              <Loader2 size={14} class="spinning" />
              Starting...
            {:else}
              <Send size={14} />
              Start Session
            {/if}
          </button>
        </div>
      </div>
    </form>
  </div>
</div>

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
  }

  .modal {
    display: flex;
    flex-direction: column;
    width: 90%;
    max-width: 500px;
    background: var(--bg-chrome);
    border-radius: 12px;
    overflow: hidden;
    box-shadow: 0 20px 40px rgba(0, 0, 0, 0.3);
  }

  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .header-content {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--text-primary);
  }

  .header-content :global(svg) {
    color: var(--text-accent);
  }

  .header-title {
    font-size: var(--size-md);
    font-weight: 500;
  }

  .close-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 6px;
    background: none;
    border: none;
    border-radius: 6px;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      color 0.1s,
      background-color 0.1s;
  }

  .close-btn:hover {
    color: var(--text-primary);
    background: var(--bg-hover);
  }

  .modal-content {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .branch-info {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    background-color: var(--bg-hover);
    border-radius: 6px;
    font-size: var(--size-sm);
  }

  .branch-info :global(svg) {
    color: var(--status-renamed);
  }

  .branch-name {
    font-weight: 500;
    color: var(--text-primary);
  }

  .repo-name {
    color: var(--text-muted);
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-group label {
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-muted);
  }

  .form-group textarea {
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    resize: vertical;
    min-height: 80px;
    transition: border-color 0.15s ease;
  }

  .form-group textarea:focus {
    outline: none;
    border-color: var(--ui-accent);
  }

  .form-group textarea::placeholder {
    color: var(--text-faint);
  }

  .prompt-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: 8px;
  }

  .attach-button {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    background: transparent;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    color: var(--text-muted);
    font-size: var(--size-sm);
    cursor: pointer;
    transition: all 0.15s;
  }

  .attach-button:hover {
    border-color: var(--border-emphasis);
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .images-preview {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .image-item {
    position: relative;
    width: 80px;
    height: 80px;
    border-radius: 6px;
    overflow: hidden;
    border: 1px solid var(--border-muted);
  }

  .image-item img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .remove-image {
    position: absolute;
    top: 4px;
    right: 4px;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    padding: 0;
    background: rgba(0, 0, 0, 0.6);
    border: none;
    border-radius: 50%;
    color: white;
    cursor: pointer;
    transition: background 0.15s;
  }

  .remove-image:hover {
    background: rgba(0, 0, 0, 0.8);
  }

  .hint {
    margin: 0;
    font-size: var(--size-xs);
    color: var(--text-faint);
    text-align: right;
  }

  .error-message {
    padding: 10px 12px;
    background: var(--ui-danger-bg);
    border-radius: 6px;
    color: var(--ui-danger);
    font-size: var(--size-sm);
  }

  .form-actions {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 10px;
    margin-top: 8px;
  }

  .action-buttons {
    display: flex;
    gap: 10px;
  }

  .cancel-btn,
  .submit-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 10px 16px;
    border-radius: 6px;
    font-size: var(--size-sm);
    font-weight: 500;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .cancel-btn {
    background: transparent;
    border: 1px solid var(--border-muted);
    color: var(--text-muted);
  }

  .cancel-btn:hover:not(:disabled) {
    border-color: var(--text-primary);
    color: var(--text-primary);
  }

  .submit-btn {
    background: var(--ui-accent);
    border: none;
    color: var(--bg-deepest);
  }

  .submit-btn:hover:not(:disabled) {
    background: var(--ui-accent-hover);
  }

  .submit-btn:disabled,
  .cancel-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  :global(.spinning) {
    animation: spin 1s linear infinite;
    transform-origin: center;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
