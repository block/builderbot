<!--
  NoteSelectionReply.svelte — "Select to reply" for a rendered note.

  Selecting text in the note shows a small Reply button by the end of the
  selection. It opens a popover with the quote (and the heading path it sits
  under) and a reply box: Enter (or "Add to reply") hands the quote and reply
  to `onAdd`, which puts them in the note chat's draft as a quote block;
  Escape or a click away cancels. The popover closes once `onAdd` reports the
  block added, and stays open with the reply if it couldn't be. Each
  selection makes one block, so a review of the whole note builds up one
  selection at a time.

  Rendered inside the note's scroll container (`scrollEl`, which must be
  positioned), so the button scrolls with the text it belongs to. While the
  popover is open the selection is frozen — focus moving into the reply box
  drops the page selection — and painted with a CSS highlight in its place.
  The quote is captured when the button appears, so the note re-rendering
  under an open popover (an agent editing it) leaves the popover, and the
  reply being typed, where they are; only the highlight goes with the old
  text.
-->
<script lang="ts">
  import { onDestroy } from 'svelte';
  import MessageSquareQuote from '@lucide/svelte/icons/message-square-quote';
  import * as Popover from '$lib/components/ui/popover';
  import { Button } from '$lib/components/ui/button';
  import QuoteExcerpt from '../sessions/QuoteExcerpt.svelte';
  import type { NewDraftQuote } from '../sessions/composerDraft';
  import { isImeKeydown } from '../../shared/imeKeydown';
  import { viewport } from '../../shared/viewport.svelte';
  import { quoteFromSelection, type NoteSelectionQuote } from './noteSelection';

  interface Props {
    /** The note's scroll container; the button is positioned in it. */
    scrollEl: HTMLElement | undefined;
    /** The rendered note, whose selections can be replied to. */
    contentEl: HTMLElement | undefined;
    /** Off while the note is hidden or has no chat to reply in. */
    enabled: boolean;
    /** Headings left out of the path (the note's title). */
    ignoreHeading?: (heading: Element) => boolean;
    /** Adds the block; resolves whether it was added. */
    onAdd: (quote: NewDraftQuote) => Promise<boolean> | boolean;
  }

  let { scrollEl, contentEl, enabled, ignoreHeading, onAdd }: Props = $props();

  /** Wait this long after the last selection change before showing the button. */
  const SETTLE_MS = 150;
  /** Width the button is kept clear of the container's edges by. */
  const EDGE_GAP = 8;
  const HIGHLIGHT_NAME = 'note-reply-selection';

  let selection = $state.raw<NoteSelectionQuote | null>(null);
  let anchor = $state<{ top: number; left: number } | null>(null);
  let open = $state(false);
  let reply = $state('');
  /** `onAdd` is still saving the block. */
  let adding = $state(false);
  let replyEl = $state<HTMLTextAreaElement | null>(null);
  let buttonEl = $state<HTMLElement | null>(null);
  let pointerDown = false;
  let settleTimer: ReturnType<typeof setTimeout> | undefined;

  function clearSelectionState() {
    selection = null;
    anchor = null;
  }

  /**
   * Where the button goes: just below the end of the selection, in scroll
   * coordinates. None for a collapsed range — the selected text was replaced,
   * and the range fell back to where it had been.
   */
  function anchorFor(range: Range, container: HTMLElement): { top: number; left: number } | null {
    if (range.collapsed) return null;
    const rects = [...range.getClientRects()].filter((rect) => rect.width > 0 && rect.height > 0);
    const last = rects[rects.length - 1] ?? range.getBoundingClientRect();
    if (last.width === 0 && last.height === 0) return null;
    const box = container.getBoundingClientRect();
    const buttonWidth = buttonEl?.offsetWidth ?? 72;
    const maxLeft = Math.max(EDGE_GAP, container.clientWidth - buttonWidth - EDGE_GAP);
    return {
      top: last.bottom - box.top + container.scrollTop + 6,
      left: Math.min(
        Math.max(last.right - box.left + container.scrollLeft - buttonWidth / 2, EDGE_GAP),
        maxLeft
      ),
    };
  }

  function readSelection() {
    clearTimeout(settleTimer);
    if (open) return;
    if (!enabled || !contentEl || !scrollEl) {
      clearSelectionState();
      return;
    }
    const next = quoteFromSelection(document.getSelection(), contentEl, { ignore: ignoreHeading });
    if (!next) {
      clearSelectionState();
      return;
    }
    selection = next;
    anchor = anchorFor(next.range, scrollEl);
  }

  function handleSelectionChange() {
    if (open) return;
    const current = document.getSelection();
    // Gone (or moved out of the note): hide at once. Still changing: wait for
    // the drag or the keyboard to settle before showing the button.
    if (!current || current.isCollapsed || !contentEl?.contains(current.anchorNode)) {
      clearTimeout(settleTimer);
      clearSelectionState();
      return;
    }
    clearTimeout(settleTimer);
    if (pointerDown) return;
    settleTimer = setTimeout(readSelection, SETTLE_MS);
  }

  function handlePointerDown(event: PointerEvent) {
    if (buttonEl?.contains(event.target as Node)) return;
    pointerDown = true;
  }

  function handlePointerUp() {
    if (!pointerDown) return;
    pointerDown = false;
    // After the browser has settled the selection the press made.
    setTimeout(readSelection, 0);
  }

  $effect(() => {
    if (!enabled) {
      open = false;
      clearSelectionState();
      return;
    }
    document.addEventListener('selectionchange', handleSelectionChange);
    document.addEventListener('pointerdown', handlePointerDown, true);
    document.addEventListener('pointerup', handlePointerUp, true);
    document.addEventListener('pointercancel', handlePointerUp, true);
    return () => {
      document.removeEventListener('selectionchange', handleSelectionChange);
      document.removeEventListener('pointerdown', handlePointerDown, true);
      document.removeEventListener('pointerup', handlePointerUp, true);
      document.removeEventListener('pointercancel', handlePointerUp, true);
      clearTimeout(settleTimer);
    };
  });

  // Text reflowing under the selection (the dialog resized, the chat pane
  // opened beside it) moves the selection's end; the button follows. The
  // note re-rendering replaces the selected text and leaves no end to
  // follow: an open popover then stays where it is rather than unmounting
  // with the reply in it.
  $effect(() => {
    const el = contentEl;
    if (!el || !enabled) return;
    const observer = new ResizeObserver(() => {
      if (!selection || !scrollEl) return;
      const next = anchorFor(selection.range, scrollEl);
      if (next || !open) anchor = next;
    });
    observer.observe(el);
    return () => observer.disconnect();
  });

  // Paint the frozen selection while the popover holds it.
  $effect(() => {
    const range = open ? selection?.range : null;
    const registry = globalThis.CSS?.highlights;
    if (!range || !registry || typeof Highlight === 'undefined') return;
    registry.set(HIGHLIGHT_NAME, new Highlight(range));
    return () => {
      registry.delete(HIGHLIGHT_NAME);
    };
  });

  onDestroy(() => clearTimeout(settleTimer));

  function handleOpenChange(next: boolean) {
    open = next;
    if (next) return;
    reply = '';
    // Cancelled: the page selection may still be there to reply to again.
    setTimeout(readSelection, 0);
  }

  async function confirm() {
    const current = selection;
    if (!current || adding) return;
    const quote: NewDraftQuote = {
      quote: current.quote,
      headings: current.headings,
      reply: reply.trim(),
    };
    adding = true;
    let added = false;
    try {
      added = await onAdd(quote);
    } catch (error) {
      console.error('[NoteSelectionReply] Failed to add the quote:', error);
    } finally {
      adding = false;
    }
    // Not added (`onAdd` has said why): keep the reply for another try.
    if (!added) {
      if (open) replyEl?.focus();
      return;
    }
    // Cancelled and another selection made while it saved: leave that one be.
    if (selection !== current) return;
    open = false;
    reply = '';
    clearSelectionState();
    document.getSelection()?.removeAllRanges();
  }

  function handleReplyKeydown(event: KeyboardEvent) {
    if (isImeKeydown(event)) return;
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      void confirm();
    }
  }
</script>

{#if selection && anchor}
  <div class="selection-reply-anchor" style:top="{anchor.top}px" style:left="{anchor.left}px">
    <Popover.Root {open} onOpenChange={handleOpenChange}>
      <Popover.Trigger
        bind:ref={buttonEl}
        class="selection-reply-button"
        title="Reply to the selected text"
        aria-label="Reply to the selected text"
        onmousedown={(event: MouseEvent) => event.preventDefault()}
      >
        <MessageSquareQuote size={12} aria-hidden="true" />
        <span>Reply</span>
      </Popover.Trigger>
      <Popover.Content
        side="bottom"
        align="start"
        sideOffset={6}
        class="w-[min(340px,calc(100vw-32px))] gap-2.5 p-3"
        onOpenAutoFocus={(event: Event) => {
          event.preventDefault();
          replyEl?.focus();
        }}
        onCloseAutoFocus={(event: Event) => event.preventDefault()}
      >
        <QuoteExcerpt quote={selection.quote} headings={selection.headings} clampLines={4} />
        <textarea
          bind:this={replyEl}
          bind:value={reply}
          class="selection-reply-input"
          rows="3"
          placeholder="Reply to this selection…"
          aria-label="Reply to the selected text"
          onkeydown={handleReplyKeydown}></textarea>
        <div class="selection-reply-actions">
          {#if viewport.showShortcutHints}
            <span class="selection-reply-hint">Enter to add · Esc to cancel</span>
          {/if}
          <Button
            variant="ghost"
            size="sm"
            class="text-muted-foreground"
            onclick={() => handleOpenChange(false)}
          >
            Cancel
          </Button>
          <Button variant="accent" size="sm" disabled={adding} onclick={confirm}>
            Add to reply
          </Button>
        </div>
      </Popover.Content>
    </Popover.Root>
  </div>
{/if}

<style>
  .selection-reply-anchor {
    position: absolute;
    z-index: 2;
  }

  .selection-reply-anchor :global(.selection-reply-button) {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 9px;
    border: 1px solid var(--border-muted);
    border-radius: 999px;
    background: var(--bg-chrome);
    color: var(--text-secondary);
    font-size: var(--size-xs);
    font-weight: 500;
    box-shadow: var(--shadow-raised);
    cursor: pointer;
    transition:
      color 0.1s,
      border-color 0.1s,
      background-color 0.1s;
  }

  .selection-reply-anchor :global(.selection-reply-button:hover),
  .selection-reply-anchor :global(.selection-reply-button[data-state='open']) {
    border-color: var(--border-emphasis);
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .selection-reply-anchor :global(.selection-reply-button:focus-visible) {
    outline: 2px solid var(--ui-accent);
    outline-offset: 1px;
  }

  .selection-reply-input {
    width: 100%;
    padding: 6px 9px;
    border: 1px solid var(--border-muted);
    border-radius: 8px;
    background: var(--bg-primary);
    color: var(--text-primary);
    font-family: inherit;
    font-size: var(--size-sm);
    line-height: 1.5;
    resize: none;
  }

  .selection-reply-input::placeholder {
    color: var(--text-faint);
  }

  .selection-reply-input:focus {
    outline: none;
    border-color: var(--border-emphasis);
  }

  .selection-reply-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
  }

  .selection-reply-hint {
    margin-right: auto;
    color: var(--text-faint);
    font-size: var(--size-xs);
  }

  /* The note's own tint, as on the rule beside each quote. */
  :global(::highlight(note-reply-selection)) {
    background-color: var(--note-bg-emphasis);
  }
</style>
