<!--
  NewSessionModal.svelte — Start a new branch session or project note session

  A focused modal with a prompt textarea. The mode (commit/note/review) is
  switchable via a clickable dropdown in the header for branch sessions. Project
  note sessions keep a fixed note mode and show a simple "Project note" header.
  On close, returns whatever text was typed and the current mode so the caller
  can restore state if the user re-opens the modal.

  Props:
    branch        — the branch to create a session on, for branch sessions
    project       — the project to create a note session on, for project sessions
    mode          — 'commit', 'note', or 'review' (initial mode)
    repoLabel     — optional repo label for display (githubRepo + subpath)
    initialPrompt — pre-fill the textarea (e.g. from a previous close)
    onClose       — called with { prompt, mode, imageIds } when dismissed
    onSubmit      — called with { prompt, mode, imageIds, provider, acpConfigSelection } when submit is pressed
-->
<script lang="ts">
  import X from '@lucide/svelte/icons/x';
  import GitCommitVertical from '@lucide/svelte/icons/git-commit-vertical';
  import FileText from '@lucide/svelte/icons/file-text';
  import FileSearch from '@lucide/svelte/icons/file-search';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import { tick, untrack } from 'svelte';
  import Spinner from '../../shared/Spinner.svelte';
  import RepoLabel from '../../shared/RepoLabel.svelte';
  import type {
    AcpConfigSelection,
    Branch,
    BranchSessionType,
    HashtagItem,
    Project,
    ProjectRepo,
  } from '../../types';
  import AcpConfigPicker from '../agents/AcpConfigPicker.svelte';
  import type { AcpConfigPickerSelection } from '../agents/acpConfigSelection';
  import ImageAttachment from './ImageAttachment.svelte';
  import HashtagInput from './HashtagInput.svelte';
  import { buildBranchHashtagItems } from './hashtagItems';
  import {
    foldSnippetsIntoPrompt,
    shouldOfferClipboardSnippet,
    snippetLabel,
    type TextSnippet,
  } from './sessionModalHelpers';
  import * as Dialog from '$lib/components/ui/dialog';
  import { Button } from '$lib/components/ui/button';
  import { subscribeDragDrop } from '../branches/dragDrop';
  import {
    isImageFile,
    isMaybeTextFile,
    insertFilePathsAtCursor,
  } from '../branches/branchCardHelpers';
  import { createImage } from '../../commands';
  import { readClipboardText } from '../../transport';
  import { viewport } from '../../shared/viewport.svelte';
  import { isMac } from '../keyboard/shortcuts';

  interface Props {
    open: boolean;
    branch?: Branch;
    project?: Project;
    mode?: BranchSessionType;
    repoLabel?: ProjectRepo | null;
    hashtagItems?: HashtagItem[];
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
    submitDisabledReason?: string | null;
    /** When true, the session will be queued rather than started immediately. */
    willQueue?: boolean;
    /** Mode-aware queue state for callers that allow switching session types. */
    willQueueForMode?: (mode: BranchSessionType) => boolean;
    onClose: (draft: { prompt: string; mode: BranchSessionType; imageIds: string[] }) => void;
    onSubmit: (data: {
      prompt: string;
      mode: BranchSessionType;
      imageIds: string[];
      provider?: string;
      acpConfigSelection?: AcpConfigSelection | null;
    }) => void;
  }

  let {
    open,
    branch = undefined,
    project = undefined,
    mode = 'note',
    repoLabel = null,
    hashtagItems: providedHashtagItems,
    initialPrompt = '',
    initialImageIds = [],
    prefilled = false,
    prefillSelection = 'all',
    commitPrefill = '',
    notePrefill = '',
    remote = false,
    submitDisabledReason = null,
    willQueue = false,
    willQueueForMode,
    onClose,
    onSubmit,
  }: Props = $props();

  let prompt = $state('');
  let currentMode = $state<BranchSessionType>('commit');
  let starting = $state(false);
  let initialized = false;
  let textareaEl: HTMLElement | null = $state(null);
  let acpPickerSelection = $state<AcpConfigPickerSelection>({
    providerId: null,
    acpConfigSelection: null,
  });

  // Text-snippet attachment state (modal-local — folded into the prompt on
  // submit, never persisted to the backend).
  let textSnippets = $state<TextSnippet[]>([]);
  // Eligible clipboard text (length > threshold), or null when nothing to offer.
  let clipboardSnippetText = $state<string | null>(null);
  // Monotonic id source for snippet chips (modal-local, no backend id).
  let snippetCounter = 0;

  let isCommit = $derived(currentMode === 'commit');
  let isReview = $derived(currentMode === 'review');
  let isNote = $derived(!isCommit && !isReview);
  let isProjectNote = $derived(!branch && !!project);
  let activeProjectId = $derived(branch?.projectId ?? project?.id ?? '');
  let activeBranchId = $derived(branch?.id ?? null);
  let activeWorkingDir = $derived(branch?.worktreePath ?? null);
  let currentWillQueue = $derived(willQueueForMode?.(currentMode) ?? willQueue);
  let canSubmit = $derived(
    !!activeProjectId &&
      !starting &&
      !submitDisabledReason &&
      (isReview || !!prompt.trim() || textSnippets.length > 0)
  );
  let submitLabel = $derived(currentWillQueue ? 'Queue' : isProjectNote ? 'New' : 'Start');
  const footerControlClass =
    'h-9 gap-1.5 rounded-md border border-[var(--border-muted)] bg-[var(--bg-primary)] px-4 py-2 text-sm font-medium text-muted-foreground shadow-none transition-colors hover:bg-[var(--bg-hover)] hover:text-foreground max-[768px]:h-11 max-[768px]:justify-center';

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

  // Platform-aware modifier label for the ⌘1/2/3 mode-switch hints.
  let modKey = $derived(isMac() ? '⌘' : 'Ctrl ');

  function switchMode(newMode: BranchSessionType) {
    if (isProjectNote) return;
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
    if (providedHashtagItems) {
      hashtagItems = providedHashtagItems;
      return;
    }
    if (!branch) {
      hashtagItems = [];
      return;
    }
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
      currentMode = isProjectNote ? 'note' : mode;
      imageIds = [...initialImageIds];
    }
  });

  // Read the clipboard on open and whenever the window regains focus so the
  // "Attach clipboard" button reflects fresh copies. Reads via the Tauri
  // plugin (more reliable in the WebView); failures simply hide the button.
  async function refreshClipboardSnippet() {
    if (!open) return;
    try {
      const text = await readClipboardText();
      clipboardSnippetText = shouldOfferClipboardSnippet(text) ? text : null;
    } catch {
      clipboardSnippetText = null;
    }
  }

  $effect(() => {
    if (!open) return;
    void refreshClipboardSnippet();
    const onFocus = () => void refreshClipboardSnippet();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  });

  function addSnippet(text: string) {
    const trimmed = text.trim();
    if (!trimmed) return;
    textSnippets = [
      ...textSnippets,
      { id: `snippet-${snippetCounter++}`, label: snippetLabel(trimmed), text: trimmed },
    ];
  }

  function attachClipboardSnippet() {
    if (!clipboardSnippetText) return;
    addSnippet(clipboardSnippetText);
    clipboardSnippetText = null;
  }

  function removeSnippet(id: string) {
    textSnippets = textSnippets.filter((s) => s.id !== id);
  }

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
    // Review mode allows empty prompts; other modes require text or a snippet.
    if (!canSubmit) return;

    starting = true;
    // Fold attached snippets into the prompt; the onSubmit signature is unchanged.
    const finalPrompt = foldSnippetsIntoPrompt(prompt.trim(), textSnippets);
    onSubmit({
      prompt: finalPrompt,
      mode: currentMode,
      imageIds,
      provider: acpPickerSelection.providerId ?? undefined,
      acpConfigSelection: acpPickerSelection.acpConfigSelection,
    });
    // Close immediately; parent handles async start + optimistic timeline row.
    onClose({ prompt: '', mode: currentMode, imageIds: [] });
  }

  function handleClose() {
    onClose({ prompt, mode: currentMode, imageIds });
  }

  function handleAcpSelectionChange(selection: AcpConfigPickerSelection) {
    acpPickerSelection = selection;
  }

  function handleKeydown(e: KeyboardEvent) {
    // Cmd/Ctrl+Enter to submit
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && canSubmit) {
      e.preventDefault();
      handleSubmit();
    }
  }

  // ⌘1 / ⌘2 / ⌘3 (Ctrl on non-Mac) switch the mode while the dialog is open,
  // whether or not the dropdown is open. Registered as a capture-phase window
  // listener so it fires before ProjectsList's bubble-phase ⌘1-9 handler
  // (which would otherwise switch the project behind the dialog); we stop
  // propagation for handled combos so that handler never sees them.
  function handleModeShortcut(e: KeyboardEvent) {
    if (!open) return;
    if (isProjectNote) return;
    const mod = isMac() ? e.metaKey : e.ctrlKey;
    if (!mod || !/^[0-9]$/.test(e.key)) return;
    const idx = Number(e.key) - 1;
    if (idx < 0 || idx >= allModes.length) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    switchMode(allModes[idx].value);
  }

  $effect(() => {
    window.addEventListener('keydown', handleModeShortcut, { capture: true });
    return () => window.removeEventListener('keydown', handleModeShortcut, { capture: true });
  });

  // =========================================================================
  // Drag-and-drop files (via Tauri native drag-drop events)
  // =========================================================================

  async function handleFileDrop(paths: string[]) {
    if (!activeProjectId) return;
    const imagePaths = paths.filter((p) => isImageFile(p));
    const textPaths = paths.filter((p) => isMaybeTextFile(p));
    const newIds: string[] = [];
    for (const path of imagePaths) {
      try {
        const image = await createImage(activeBranchId, activeProjectId, path, true);
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

  type ImageIdsUpdate = string[] | ((current: string[]) => string[]);

  // Keep imageIds in sync with ImageAttachment changes
  function onImageIdsChange(update: ImageIdsUpdate) {
    imageIds = typeof update === 'function' ? update(imageIds) : update;
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
    <Dialog.Title class="sr-only">
      {isProjectNote ? 'Project note session' : `New ${currentModeInfo.label} session`}
    </Dialog.Title>
    <header class="modal-header">
      <div class="header-left">
        {#if isProjectNote}
          <div class="project-note-heading">Project note</div>
        {:else}
          <div class="mode-switcher" bind:this={modeMenuEl}>
            <button
              class={`mode-switcher-btn ${currentModeInfo.iconClass}`}
              onclick={() => (modeMenuOpen = !modeMenuOpen)}
              type="button"
            >
              <currentModeInfo.icon size={14} />
              <span>{currentModeInfo.label}</span>
              <ChevronDown size={14} />
            </button>
            {#if modeMenuOpen}
              <div class="mode-menu">
                {#each allModes as m, i}
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
                    {#if viewport.showShortcutHints}
                      <span class="mode-menu-hint">{modKey}{i + 1}</span>
                    {/if}
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
        {#if !isProjectNote && repoLabel}
          <div class="repo-info">
            <RepoLabel
              githubRepo={repoLabel.headRepo ?? repoLabel.githubRepo}
              subpath={repoLabel.subpath}
            />
          </div>
        {/if}
      </div>
      <Button
        variant="ghost"
        size="icon"
        class="size-7 shrink-0 text-muted-foreground hover:bg-[var(--bg-hover)] hover:text-foreground max-[768px]:size-10 [&_svg]:!size-[18px]"
        title={viewport.showShortcutHints ? 'Close (Esc)' : 'Close'}
        aria-label="Close"
        onclick={handleClose}
      >
        <X size={18} />
      </Button>
    </header>

    <form class="modal-body" onsubmit={handleSubmit}>
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
      </div>

      {#if imageIds.length > 0 || textSnippets.length > 0}
        <ImageAttachment
          branchId={activeBranchId}
          projectId={activeProjectId}
          disabled={starting}
          {imageIds}
          {onImageIdsChange}
          {textSnippets}
          onRemoveSnippet={removeSnippet}
          clipboardText={clipboardSnippetText}
          onAttachClipboard={attachClipboardSnippet}
        />
      {/if}

      <div class="form-actions">
        <div class="form-actions-left">
          <AcpConfigPicker
            disabled={starting}
            {remote}
            dropUp
            triggerClass={footerControlClass}
            workingDir={activeWorkingDir}
            onSelectionChange={handleAcpSelectionChange}
          />
          {#if imageIds.length === 0 && textSnippets.length === 0}
            <ImageAttachment
              branchId={activeBranchId}
              projectId={activeProjectId}
              disabled={starting}
              {imageIds}
              {onImageIdsChange}
              {textSnippets}
              onRemoveSnippet={removeSnippet}
              clipboardText={clipboardSnippetText}
              onAttachClipboard={attachClipboardSnippet}
            />
          {/if}
        </div>
        <div class="form-actions-right">
          <Button
            type="button"
            variant="outline"
            class="gap-1.5 px-4 py-2 text-sm font-medium text-muted-foreground hover:text-foreground max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
            onclick={handleClose}
            disabled={starting}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            variant="accent"
            class="gap-1.5 px-4 py-2 text-sm max-[768px]:h-11 max-[768px]:flex-1 max-[768px]:justify-center"
            title={submitDisabledReason ?? undefined}
            disabled={!canSubmit}
          >
            {#if starting}
              <Spinner size={14} />
              {currentWillQueue ? 'Queueing…' : 'Starting…'}
            {:else}
              {submitLabel}
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
    gap: 8px;
    padding: 10px 18px;
    border-bottom: 1px solid var(--border-subtle);
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    flex: 1;
  }

  .mode-switcher {
    position: relative;
    flex-shrink: 0;
  }

  .project-note-heading {
    color: var(--text-primary);
    font-size: var(--size-sm);
    font-weight: 650;
    line-height: 1.4;
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
    padding: 4px 10px;
    cursor: pointer;
    transition: all 0.1s;
  }

  /* The whole trigger reads as a single coloured pill that tracks the mode. */
  .mode-switcher-btn.note-icon {
    background: var(--note-bg);
    color: var(--note-color);
  }

  .mode-switcher-btn.commit-icon {
    background: var(--commit-bg);
    color: var(--commit-color);
  }

  .mode-switcher-btn.review-icon {
    background: var(--review-bg);
    color: var(--review-color);
  }

  .mode-switcher-btn.note-icon:hover {
    background: var(--note-bg-emphasis);
  }

  .mode-switcher-btn.commit-icon:hover {
    background: var(--commit-bg-emphasis);
  }

  .mode-switcher-btn.review-icon:hover {
    background: var(--review-bg-emphasis);
  }

  /* Both glyphs (mode icon + chevron) inherit the pill's tint. */
  .mode-switcher-btn > :global(svg:last-child) {
    color: inherit;
    opacity: 0.7;
    margin-left: 2px;
  }

  .mode-menu {
    position: absolute;
    top: 100%;
    left: 0;
    margin-top: 4px;
    background: var(--bg-menu);
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    box-shadow: var(--shadow-elevated);
    padding: 4px;
    z-index: 10;
    min-width: 200px;
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

  .mode-menu-hint {
    margin-left: auto;
    padding-left: 12px;
    font-size: var(--size-xs);
    font-weight: 500;
    color: var(--text-faint);
    letter-spacing: 0.04em;
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

  .repo-info {
    min-width: 0;
    padding: 4px 10px;
    background: var(--bg-hover);
    border-radius: 6px;
    font-size: var(--size-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    /* Keep scroll momentum inside the prompt editor so it doesn't chain to the
       page behind the keyboard-shrunk full-screen dialog. */
    overscroll-behavior: contain;
    transition: border-color 0.15s;
  }

  .form-group :global(.hashtag-editor):focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  /* Actions */
  .form-actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-top: 4px;
    /* Stay pinned at the bottom of the keyboard-shrunk dialog while the prompt
       editor above is the only scroller. */
    flex-shrink: 0;
  }

  .form-actions-left {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .form-actions-right {
    display: flex;
    gap: 8px;
  }

  @media (max-width: 768px) {
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

    .form-actions-left {
      flex-wrap: wrap;
      width: 100%;
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
