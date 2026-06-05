<!--
  NewSessionModal.svelte — Start a new branch session on a branch

  A focused modal with a prompt textarea. The mode (commit/note/review) is
  switchable via a clickable dropdown in the header. On close, returns
  whatever text was typed and the current mode so the caller can restore
  state if the user re-opens the modal.

  Props:
    branch        — the branch to create a session on
    mode          — 'commit', 'note', or 'review' (initial mode)
    repoLabel     — optional repo label for display (githubRepo + subpath)
    initialPrompt — pre-fill the textarea (e.g. from a previous close)
    onClose       — called with { prompt, mode, imageIds } when dismissed
    onSubmit      — called with { prompt, mode, imageIds } when submit is pressed
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
  import FileText from '@lucide/svelte/icons/file-text';
  import FileSearch from '@lucide/svelte/icons/file-search';
  import Send from '@lucide/svelte/icons/send';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import { tick, untrack } from 'svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import type { Branch, BranchSessionType, HashtagItem, ProjectRepo } from '../../types';
  import AgentSelector from '../agents/AgentSelector.svelte';
  import ImageAttachment from './ImageAttachment.svelte';
  import HashtagInput from './HashtagInput.svelte';
  import { buildBranchHashtagItems } from './hashtagItems';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import * as Tooltip from '$lib/components/ui/tooltip';
  import { subscribeDragDrop } from '../branches/dragDrop';
  import {
    isImageFile,
    isMaybeTextFile,
    insertFilePathsAtCursor,
  } from '../branches/branchCardHelpers';
  import { createImage } from '../../commands';
  import { viewport } from '../../shared/viewport.svelte';

  interface Props {
    open: boolean;
    branch: Branch;
    mode: BranchSessionType;
    repoLabel?: ProjectRepo | null;
    initialPrompt?: string;
    initialImageIds?: string[];
    /** When true, the initial prompt is a suggestion — select text according to prefillSelection. */
    prefilled?: boolean;
    /** Which part of the initial prompt should be selected when prefilled is true. */
    prefillSelection?: 'all' | 'last-line';
    /** Commit-mode prefill text — used when switching to commit mode with no user draft. */
    commitPrefill?: string;
    /** Note-mode prefill text — used when switching to note mode with no user draft. */
    notePrefill?: string;
    remote?: boolean;
    /** When true, the session will be queued rather than started immediately. */
    willQueue?: boolean;
    onClose: (draft: { prompt: string; mode: BranchSessionType; imageIds: string[] }) => void;
    onSubmit: (data: { prompt: string; mode: BranchSessionType; imageIds: string[] }) => void;
  }

  let {
    open,
    branch,
    mode,
    repoLabel = null,
    initialPrompt = '',
    initialImageIds = [],
    prefilled = false,
    prefillSelection = 'all',
    commitPrefill = '',
    notePrefill = '',
    remote = false,
    willQueue = false,
    onClose,
    onSubmit,
  }: Props = $props();

  let prompt = $state('');
  let currentMode = $state<BranchSessionType>('commit');
  let starting = $state(false);
  let initialized = false;
  let textareaEl: HTMLElement | null = $state(null);

  let isCommit = $derived(currentMode === 'commit');
  let isReview = $derived(currentMode === 'review');
  let isNote = $derived(!isCommit && !isReview);

  function selectPromptContent(el: HTMLElement, selection: 'all' | 'last-line') {
    const sel = window.getSelection();
    if (!sel) return;

    const range = document.createRange();
    if (selection === 'last-line') {
      const start = findLastLineStart(el);
      range.setStart(start.node, start.offset);
      range.setEnd(el, el.childNodes.length);
    } else {
      range.selectNodeContents(el);
    }
    sel.removeAllRanges();
    sel.addRange(range);
  }

  function findLastLineStart(el: HTMLElement): { node: Node; offset: number } {
    let start: { node: Node; offset: number } = { node: el, offset: 0 };

    function visit(node: Node) {
      if (node instanceof HTMLBRElement) {
        const parent = node.parentNode;
        if (parent) {
          start = {
            node: parent,
            offset: Array.prototype.indexOf.call(parent.childNodes, node) + 1,
          };
        }
        return;
      }

      for (const child of node.childNodes) {
        visit(child);
      }
    }

    visit(el);
    return start;
  }

  function placeCursorAtEnd(el: HTMLElement) {
    const sel = window.getSelection();
    if (!sel) return;

    const range = document.createRange();
    range.selectNodeContents(el);
    range.collapse(false);
    sel.removeAllRanges();
    sel.addRange(range);
  }

  // Animated placeholder for note mode — cycles through example prompts
  const notePlaceholders = [
    'Plan a feature that…',
    'Research how this works…',
    'Look into why this bug could occur…',
  ];
  let notePlaceholderIndex = $state(0);
  let notePlaceholderCharIndex = $state(0);
  let notePlaceholderPhase = $state<'typing' | 'holding' | 'erasing'>('typing');

  let notePlaceholder = $derived(
    notePlaceholders[notePlaceholderIndex].slice(0, notePlaceholderCharIndex)
  );

  $effect(() => {
    if (!isNote || prompt) return;
    const text = notePlaceholders[notePlaceholderIndex];
    let delay: number;
    if (notePlaceholderPhase === 'typing') {
      delay = notePlaceholderCharIndex < text.length ? 40 : -1;
      if (delay === -1) {
        // Done typing — hold before erasing
        const t = setTimeout(() => {
          notePlaceholderPhase = 'erasing';
        }, 2000);
        return () => clearTimeout(t);
      }
    } else if (notePlaceholderPhase === 'erasing') {
      delay = notePlaceholderCharIndex > 0 ? 20 : -1;
      if (delay === -1) {
        // Done erasing — move to next prompt
        const t = setTimeout(() => {
          notePlaceholderIndex = (notePlaceholderIndex + 1) % notePlaceholders.length;
          notePlaceholderPhase = 'typing';
        }, 400);
        return () => clearTimeout(t);
      }
    } else {
      return;
    }
    const step = notePlaceholderPhase === 'typing' ? 1 : -1;
    const t = setTimeout(() => {
      notePlaceholderCharIndex += step;
    }, delay);
    return () => clearTimeout(t);
  });

  // Mode switcher dropdown state
  let modeMenuOpen = $state(false);
  let modeMenuEl: HTMLDivElement | undefined = $state();

  const allModes: {
    value: BranchSessionType;
    label: string;
    icon: typeof GitCommitVertical;
    iconClass: string;
  }[] = [
    { value: 'note', label: 'New note', icon: FileText, iconClass: 'note-icon' },
    { value: 'commit', label: 'New commit', icon: GitCommitVertical, iconClass: 'commit-icon' },
    { value: 'review', label: 'New code review', icon: FileSearch, iconClass: 'review-icon' },
  ];

  let currentModeInfo = $derived(allModes.find((m) => m.value === currentMode)!);

  function switchMode(newMode: BranchSessionType) {
    modeMenuOpen = false;
    if (newMode === currentMode) return;

    // Replicate the close-and-reopen prefill behaviour:
    // If the prompt is still a prefill (user didn't type anything custom),
    // treat it as empty so prefill can be reconsidered for the new mode.
    const isPromptPrefill =
      (commitPrefill && prompt === commitPrefill) || (notePrefill && prompt === notePrefill);
    const userDraft = isPromptPrefill ? '' : prompt;

    currentMode = newMode;

    // Apply prefill when switching to commit or note mode with no user draft
    const modePrefill =
      newMode === 'commit' ? commitPrefill : newMode === 'note' ? notePrefill : '';
    if (modePrefill && !userDraft) {
      prompt = modePrefill;
      const modeSelection = modePrefill.includes('\n') ? 'last-line' : prefillSelection;
      tick().then(() => {
        if (textareaEl && textareaEl.textContent) {
          selectPromptContent(textareaEl, modeSelection);
          textareaEl.focus();
        }
      });
    } else {
      prompt = userDraft;
    }
  }

  // Close mode menu on outside click
  function handleDocumentClick(e: MouseEvent) {
    if (modeMenuOpen && modeMenuEl && !modeMenuEl.contains(e.target as Node)) {
      modeMenuOpen = false;
    }
  }

  // Hashtag reference items
  let hashtagItems = $state<HashtagItem[]>([]);
  $effect(() => {
    let stale = false;
    buildBranchHashtagItems(branch.id, branch.projectId, {
      branchName: branch.branchName,
      repoSlug: repoLabel?.headRepo ?? repoLabel?.githubRepo,
      repoSubpath: repoLabel?.subpath,
    }).then((items) => {
      if (!stale) hashtagItems = items;
    });
    return () => {
      stale = true;
    };
  });

  // Image attachment state
  let imageIds = $state<string[]>([]);

  // Drag-and-drop state
  let dragOver = $state(false);
  let modalElement: HTMLElement | null = $state(null);

  // Seed prompt and mode from props once; caller preserves draft across open/close.
  $effect(() => {
    if (!initialized) {
      initialized = true;
      prompt = initialPrompt;
      currentMode = mode;
      imageIds = [...initialImageIds];
    }
  });

  // Focus textarea on mount (one-time).
  // We await tick() so the DOM reflects the prompt value set by the init effect above.
  $effect(() => {
    if (textareaEl) {
      const el = textareaEl;
      const shouldSelect = prefilled;
      tick().then(() => {
        el.focus();
        if (shouldSelect && el.textContent) {
          selectPromptContent(el, prefillSelection);
        } else {
          placeCursorAtEnd(el);
        }
      });
    }
  });

  // Resolving hashtag references can re-render the editor. Keep diff-launched
  // prompts focused on the editable action line as long as the prefill is untouched.
  $effect(() => {
    const _hashtagItems = hashtagItems;
    if (!textareaEl || !prefilled || prefillSelection !== 'last-line' || prompt !== initialPrompt) {
      return;
    }

    tick().then(() => {
      if (textareaEl && document.activeElement === textareaEl && prompt === initialPrompt) {
        selectPromptContent(textareaEl, 'last-line');
      }
    });
  });

  function handleSubmit(e?: Event) {
    e?.preventDefault();
    // Review mode allows empty prompts; other modes require text
    if (!isReview && !prompt.trim()) return;
    if (starting) return;

    starting = true;
    onSubmit({ prompt: prompt.trim(), mode: currentMode, imageIds });
    // Close immediately; parent handles async start + optimistic timeline row.
    onClose({ prompt: '', mode: currentMode, imageIds: [] });
  }

  function handleClose() {
    onClose({ prompt, mode: currentMode, imageIds });
  }

  function handleKeydown(e: KeyboardEvent) {
    // Cmd+Enter to submit
    if (e.key === 'Enter' && e.metaKey && (prompt.trim() || isReview) && !starting) {
      e.preventDefault();
      handleSubmit();
    }
  }

  // =========================================================================
  // Drag-and-drop files (via Tauri native drag-drop events)
  // =========================================================================

  async function handleFileDrop(paths: string[]) {
    const imagePaths = paths.filter((p) => isImageFile(p));
    const textPaths = paths.filter((p) => isMaybeTextFile(p));
    const newIds: string[] = [];
    for (const path of imagePaths) {
      try {
        const image = await createImage(branch.id, branch.projectId, path, true);
        newIds.push(image.id);
      } catch (e) {
        console.error('Failed to create image from dropped file:', e);
      }
    }
    if (newIds.length > 0) {
      imageIds = [...imageIds, ...newIds];
      onImageIdsChange(imageIds);
    }
    if (textPaths.length > 0 && textareaEl) {
      insertFilePathsAtCursor(textareaEl, textPaths);
    }
  }

  // Keep imageIds in sync with ImageAttachment changes
  function onImageIdsChange(ids: string[]) {
    imageIds = ids;
  }

  // Subscribe to the shared drag-drop service so the modal intercepts
  // drags that would otherwise land on the branch card behind it.
  $effect(() => {
    const el = modalElement;
    if (!el) return;

    const unsub = untrack(() =>
      subscribeDragDrop({
        element: el,
        blocking: true,
        onDragOver: (over) => {
          dragOver = over;
        },
        onDrop: (paths) => {
          handleFileDrop(paths);
        },
      })
    );

    return unsub;
  });
</script>

<svelte:window onkeydown={handleKeydown} onclick={handleDocumentClick} />

<Dialog.Root {open} onOpenChange={(v) => !v && handleClose()}>
  <Dialog.Content
    bind:ref={modalElement}
    class={`sm:max-w-[580px] max-h-[calc(100vh-16vh)] p-0 gap-0 overflow-hidden flex flex-col border-2 ${dragOver ? 'border-[var(--ui-accent)] bg-[color-mix(in_srgb,var(--ui-accent)_5%,var(--bg-chrome))]' : 'border-transparent'} transition-colors`}
    showCloseButton={false}
  >
    <Dialog.Title class="sr-only">New {currentModeInfo.label} session</Dialog.Title>
    <header class="modal-header">
      <div class="mode-switcher" bind:this={modeMenuEl}>
        <button
          class="mode-switcher-btn"
          onclick={() => (modeMenuOpen = !modeMenuOpen)}
          type="button"
        >
          <span class="header-icon {currentModeInfo.iconClass}">
            <currentModeInfo.icon size={14} />
          </span>
          <span>{currentModeInfo.label}</span>
          <ChevronDown size={14} />
        </button>
        {#if modeMenuOpen}
          <div class="mode-menu">
            {#each allModes as m}
              <button
                class="mode-menu-item"
                class:active={m.value === currentMode}
                type="button"
                onclick={() => switchMode(m.value)}
              >
                <span class="header-icon {m.iconClass}">
                  <m.icon size={14} />
                </span>
                <span>{m.label}</span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
      <Tooltip.Root>
        <Tooltip.Trigger>
          {#snippet child({ props })}
            <Button
              {...props}
              variant="ghost"
              size="icon"
              class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[640px]:size-10 [&_svg]:!size-[18px]"
              onclick={handleClose}
            >
              <X size={18} />
            </Button>
          {/snippet}
        </Tooltip.Trigger>
        <Tooltip.Content>{viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}</Tooltip.Content>
      </Tooltip.Root>
    </header>

    <form class="modal-body" onsubmit={handleSubmit}>
      {#if repoLabel}
        <div class="repo-info">
          <RepoLabel
            githubRepo={repoLabel.headRepo ?? repoLabel.githubRepo}
            subpath={repoLabel.subpath}
          />
        </div>
      {/if}

      <div class="form-group">
        <HashtagInput
          bind:textareaEl
          bind:value={prompt}
          placeholder={isReview
            ? 'Optional: focus the review on specific areas…'
            : isCommit
              ? 'Describe the change…'
              : notePlaceholder}
          rows={12}
          disabled={starting}
          items={hashtagItems}
        />
        {#if viewport.showShortcutHints}
          <span class="hint">{willQueue ? '⌘ Enter to queue' : '⌘ Enter to start'}</span>
        {/if}
      </div>

      <ImageAttachment
        branchId={branch.id}
        projectId={branch.projectId}
        disabled={starting}
        {imageIds}
        {onImageIdsChange}
      />

      <div class="form-actions">
        <AgentSelector disabled={starting} {remote} dropUp />
        <div class="form-actions-right">
          <Button
            type="button"
            variant="outline"
            class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground max-[640px]:h-11 max-[640px]:flex-1 max-[640px]:justify-center"
            onclick={handleClose}
            disabled={starting}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            variant="outline"
            class="gap-1.5 px-4 py-2 text-sm font-medium max-[640px]:h-11 max-[640px]:flex-1 max-[640px]:justify-center"
            disabled={starting || (!isReview && !prompt.trim())}
          >
            {#if starting}
              <Spinner size={14} />
              {willQueue ? 'Queueing…' : 'Starting…'}
            {:else}
              <Send size={14} />
              {willQueue ? 'Queue' : 'Start'}
            {/if}
          </Button>
        </div>
      </div>
    </form>
  </Dialog.Content>
</Dialog.Root>

<style>
  /* Header */
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 18px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .mode-switcher {
    position: relative;
  }

  .mode-switcher-btn {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: var(--size-sm);
    font-weight: 600;
    color: var(--text-primary);
    background: none;
    border: 1px solid transparent;
    border-radius: 6px;
    padding: 4px 8px;
    cursor: pointer;
    transition: all 0.1s;
  }

  .mode-switcher-btn:hover {
    background: var(--bg-hover);
    border-color: var(--border-subtle);
  }

  .mode-switcher-btn > :global(svg:last-child) {
    color: var(--text-muted);
    margin-left: 2px;
  }

  .mode-menu {
    position: absolute;
    top: 100%;
    left: 0;
    margin-top: 4px;
    background: var(--bg-chrome);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    padding: 4px;
    z-index: 10;
    min-width: 180px;
  }

  .mode-menu-item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    padding: 6px 10px;
    font-size: var(--size-sm);
    font-weight: 500;
    color: var(--text-primary);
    background: none;
    border: none;
    border-radius: 6px;
    cursor: pointer;
    transition: background-color 0.1s;
  }

  .mode-menu-item:hover {
    background: var(--bg-hover);
  }

  .mode-menu-item.active {
    background: var(--bg-hover);
  }

  .header-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    border-radius: 4px;
    flex-shrink: 0;
  }

  .header-icon.note-icon {
    background-color: var(--note-bg);
    color: var(--note-color);
  }

  .header-icon.commit-icon {
    background-color: var(--commit-bg);
    color: var(--commit-color);
  }

  .header-icon.review-icon {
    background-color: var(--review-bg);
    color: var(--review-color);
  }

  .header-icon :global(svg) {
    flex-shrink: 0;
  }

  /* Body */
  .modal-body {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
    min-height: 0;
  }

  .repo-info {
    padding: 8px 10px;
    background: var(--bg-hover);
    border-radius: 6px;
    font-size: var(--size-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    min-height: 0;
  }

  /* Propagate flex constraint through HashtagInput wrapper divs */
  .form-group :global(.hashtag-input-wrapper) {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .form-group :global(.hashtag-input-container) {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .form-group :global(.hashtag-editor) {
    padding: 10px 12px;
    background: var(--bg-primary);
    border: 1px solid var(--border-muted);
    border-radius: 6px;
    color: var(--text-primary);
    font-size: var(--size-md);
    font-family: inherit;
    line-height: 1.5;
    flex: 1;
    overflow-y: auto;
    transition: border-color 0.15s;
  }

  .form-group :global(.hashtag-editor):focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  .hint {
    font-size: var(--size-xs);
    color: var(--text-faint);
    text-align: right;
  }

  /* Actions */
  .form-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 4px;
  }

  .form-actions-right {
    display: flex;
    gap: 8px;
  }

  @media (max-width: 640px) {
    .modal-header {
      padding: 12px 16px;
    }

    .mode-switcher-btn {
      min-height: 40px;
    }

    .modal-body {
      padding: 16px;
    }

    .form-actions {
      align-items: stretch;
      flex-direction: column;
    }

    .form-actions :global(.selector-btn) {
      min-height: 40px;
      max-width: calc(100vw - 32px);
    }

    .form-actions :global(.selector-label) {
      overflow: hidden;
      text-overflow: ellipsis;
    }

    .form-actions-right {
      width: 100%;
    }
  }
</style>
