<!--
  NoteReplyQuotes.svelte — a note's quoted replies, in the note.

  The note's chat draft (sessions/replyDrafts.ts) holds a quote block for
  each passage the reader replied to. This puts the two halves of that in
  the rendered note and joins them to the draft:

  - `NoteSelectionReply`: selecting text offers a Reply button, and the
    reply lands in the draft as a new block — straight into the composer
    when the chat shows beside the note (`chatVisible`), otherwise typed in
    a field by the selection and written as it goes.
  - `NoteQuoteHighlights`: every passage the draft quotes stays marked for
    as long as its block is in the draft, and pressing a mark goes back to
    its reply. Beside the note the composer has the block, so the caret
    lands in its reply there; otherwise the reply field reopens by the
    passage with what was written, to edit or remove.

  Rendered inside the note's scroll container, as NoteSelectionReply must
  be. A draft write that fails is reported in a toast.
-->
<script lang="ts">
  import { toast } from 'svelte-sonner';
  import type { DraftQuote, NewDraftQuote } from '../sessions/composerDraft';
  import {
    addQuoteToReplyDraft,
    editLiveQuoteReply,
    focusReplyDraftQuote,
    replyDraftPresence,
    startLiveQuoteReply,
    type LiveQuoteReply,
  } from '../sessions/replyDrafts';
  import NoteQuoteHighlights from './NoteQuoteHighlights.svelte';
  import NoteSelectionReply from './NoteSelectionReply.svelte';

  interface Props {
    /** The session whose chat the note is replied to in. */
    sessionId: string;
    /** The note's scroll container (positioned); the Reply button sits in it. */
    scrollEl: HTMLElement | undefined;
    /** The rendered note. */
    contentEl: HTMLElement | undefined;
    /** What the note was rendered from: its marks are put back on every new render. */
    renderKey: unknown;
    /** Bumped when something else rewrote the note's text under the marks (find-in-note). */
    refreshKey?: unknown;
    /** Off while the note is hidden or not rendered. */
    enabled: boolean;
    /** The chat is open, beside the note or over it. */
    chatOpen: boolean;
    /** The chat composer shows the draft beside the note. */
    chatVisible: boolean;
    /** On a phone, where the reply sheet and the docked Reply button go (see NoteSelectionReply). */
    sheetHost?: HTMLElement | null;
    dockHost?: HTMLElement | null;
    onDockedChange?: (docked: boolean) => void;
    /** Headings left out of a quote's heading path (the note's title). */
    ignoreHeading?: (heading: Element) => boolean;
  }

  let {
    sessionId,
    scrollEl,
    contentEl,
    renderKey,
    refreshKey = null,
    enabled,
    chatOpen,
    chatVisible,
    sheetHost = null,
    dockHost = null,
    onDockedChange,
    ignoreHeading,
  }: Props = $props();

  /** The draft's quote blocks, whose passages the note marks. */
  let quotes = $derived(replyDraftPresence.quotes(sessionId));
  /** The quotes the note found and marked, and the one whose reply field is open. */
  let markedQuoteIds = $state.raw<string[]>([]);
  let activeQuoteId = $state<string | null>(null);
  let selectionReply = $state<ReturnType<typeof NoteSelectionReply>>();

  /**
   * With the composer showing beside the note, a selected passage goes
   * straight into it as a quote block and the caret lands in the block's
   * reply. Returns whether it was added: on failure the Reply button stays
   * for another try.
   */
  async function handleQuoteAdd(quote: NewDraftQuote): Promise<boolean> {
    const sid = sessionId;
    let id: string;
    try {
      id = await addQuoteToReplyDraft(sid, quote);
    } catch (error) {
      console.error('Failed to add quote to reply draft', error);
      toast.error('Could not add the quote to your reply draft');
      return false;
    }
    focusReplyDraftQuote(sid, id);
    return true;
  }

  /**
   * With the chat hidden, the reply to a selected passage is typed in the
   * note and the draft follows it (see replyDrafts.startLiveQuoteReply).
   * Closing the field leaves the user on the note, whatever the width, so
   * the next passage can be selected without the view changing underfoot;
   * the note's bottom bar offers the chat while the draft holds anything.
   */
  function handleQuoteReply(quote: NewDraftQuote): LiveQuoteReply {
    return reporting(startLiveQuoteReply(sessionId, quote));
  }

  /**
   * The same field reopened on a block already in the draft (its marked
   * passage was pressed): what is typed updates that block.
   */
  function handleQuoteEdit(quote: DraftQuote): LiveQuoteReply {
    return reporting(editLiveQuoteReply(sessionId, quote.id, quote.reply));
  }

  /** `live`, telling the user when the draft couldn't be written. */
  function reporting(live: LiveQuoteReply): LiveQuoteReply {
    return {
      id: live.id,
      setReply: live.setReply,
      finish: async () => {
        try {
          await live.finish();
        } catch (error) {
          console.error('Failed to save the reply draft', error);
          toast.error('Could not save your reply draft');
        }
      },
      remove: async () => {
        try {
          await live.remove();
        } catch (error) {
          console.error('Failed to remove the quote from the reply draft', error);
          toast.error('Could not remove the quote from your reply draft');
        }
      },
    };
  }

  /** A marked passage was pressed: back to its reply, wherever that is written. */
  function handleMarkActivate(quoteId: string, range: Range, stop: HTMLElement | null) {
    const quote = quotes.find((candidate) => candidate.id === quoteId);
    if (!quote) return;
    if (chatVisible) {
      focusReplyDraftQuote(sessionId, quoteId);
      return;
    }
    selectionReply?.editQuote(quote, range, stop);
  }
</script>

<NoteQuoteHighlights
  {contentEl}
  {renderKey}
  {refreshKey}
  {quotes}
  {activeQuoteId}
  {ignoreHeading}
  onMarked={(quoteIds) => (markedQuoteIds = quoteIds)}
  onActivate={handleMarkActivate}
/>
<NoteSelectionReply
  bind:this={selectionReply}
  {scrollEl}
  {contentEl}
  {enabled}
  {chatOpen}
  {chatVisible}
  {sheetHost}
  {dockHost}
  {onDockedChange}
  {ignoreHeading}
  onAdd={handleQuoteAdd}
  onReply={handleQuoteReply}
  onEdit={handleQuoteEdit}
  {markedQuoteIds}
  onActiveQuoteChange={(quoteId) => (activeQuoteId = quoteId)}
/>
